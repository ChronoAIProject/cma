use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
};
use std::time::{Duration, Instant, SystemTime};

use axum::body::Body;
use axum::extract::{Extension, Form, State};
use axum::http::{header, HeaderMap, Request, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use layer2_core::{Authorization, Capability, SubjectId};
use tokio::sync::Notify;
use tower::ServiceExt;

use super::persistence::{
    AuthStore, BrowserSession, DurableSubjectTokens, MemoryAuthStore, SecretDigest, SecretValue,
    SubjectAuthState, SubjectRefreshLease,
};
use super::{
    AuthMode, AuthOperationBudget, AuthState, AuthorizationCheckOutcome,
    AUTHORIZATION_CHECK_ATTEMPT_BUDGET, SESSION_COOKIE,
};

#[derive(Clone)]
enum IntrospectionBody {
    Active(bool),
    SubjectMismatch,
    MissingActive,
    MissingPermissions,
    MalformedPermissions,
    Malformed,
}

#[derive(Clone)]
struct ProviderState {
    base: String,
    userinfo_status: StatusCode,
    userinfo_subject: Option<String>,
    introspection_status: StatusCode,
    introspection_body: IntrospectionBody,
    permissions: Vec<String>,
    seen_userinfo_tokens: Arc<Mutex<Vec<String>>>,
    seen_introspection_tokens: Arc<Mutex<Vec<String>>>,
    introspection_calls: Arc<AtomicUsize>,
}

async fn discovery(State(provider): State<ProviderState>) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "authorization_endpoint": format!("{}/authorize", provider.base),
        "token_endpoint": format!("{}/token", provider.base),
        "userinfo_endpoint": format!("{}/userinfo", provider.base),
        "introspection_endpoint": format!("{}/introspect", provider.base),
    }))
}

async fn userinfo(State(provider): State<ProviderState>, headers: HeaderMap) -> Response {
    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .unwrap_or_default()
        .to_string();
    provider
        .seen_userinfo_tokens
        .lock()
        .expect("userinfo token log")
        .push(token.clone());
    if !provider.userinfo_status.is_success() {
        return provider.userinfo_status.into_response();
    }
    let subject = provider.userinfo_subject.unwrap_or_else(|| {
        token
            .strip_prefix("token:")
            .and_then(|value| value.split(':').next())
            .unwrap_or("subject-a")
            .to_string()
    });
    Json(serde_json::json!({
        "sub": subject,
        "permissions": provider.permissions,
    }))
    .into_response()
}

async fn introspection(
    State(provider): State<ProviderState>,
    Form(form): Form<HashMap<String, String>>,
) -> Response {
    provider.introspection_calls.fetch_add(1, Ordering::SeqCst);
    provider
        .seen_introspection_tokens
        .lock()
        .expect("introspection token log")
        .push(form.get("token").cloned().unwrap_or_default());
    if !provider.introspection_status.is_success() {
        return provider.introspection_status.into_response();
    }
    match provider.introspection_body {
        IntrospectionBody::Active(active) => Json(serde_json::json!({
            "active": active,
            "permissions": provider.permissions,
        }))
        .into_response(),
        IntrospectionBody::SubjectMismatch => Json(serde_json::json!({
            "active": true,
            "sub": "another-subject",
            "permissions": provider.permissions,
        }))
        .into_response(),
        IntrospectionBody::MissingActive => Json(serde_json::json!({
            "permissions": provider.permissions,
        }))
        .into_response(),
        IntrospectionBody::MissingPermissions => Json(serde_json::json!({
            "active": true,
        }))
        .into_response(),
        IntrospectionBody::MalformedPermissions => Json(serde_json::json!({
            "active": true,
            "permissions": ["cma:session:use", 7],
        }))
        .into_response(),
        IntrospectionBody::Malformed => (StatusCode::OK, "not-json").into_response(),
    }
}

async fn provider(
    userinfo_status: StatusCode,
    introspection_status: StatusCode,
    introspection_body: IntrospectionBody,
    permissions: &[&str],
    userinfo_subject: Option<&str>,
) -> (ProviderState, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind provider");
    let base = format!(
        "http://{}",
        listener.local_addr().expect("provider address")
    );
    let state = ProviderState {
        base,
        userinfo_status,
        userinfo_subject: userinfo_subject.map(str::to_string),
        introspection_status,
        introspection_body,
        permissions: permissions
            .iter()
            .map(|value| (*value).to_string())
            .collect(),
        seen_userinfo_tokens: Arc::new(Mutex::new(Vec::new())),
        seen_introspection_tokens: Arc::new(Mutex::new(Vec::new())),
        introspection_calls: Arc::new(AtomicUsize::new(0)),
    };
    let app = Router::new()
        .route("/.well-known/openid-configuration", get(discovery))
        .route("/userinfo", get(userinfo))
        .route("/introspect", post(introspection))
        .with_state(state.clone());
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve provider");
    });
    (state, server)
}

#[derive(Clone)]
struct OrderedProviderState {
    base: String,
    calls: Arc<AtomicUsize>,
    first_active: bool,
    first_started: Arc<Notify>,
    release_first: Arc<Notify>,
    refresh_started: Arc<Notify>,
    release_refresh: Arc<Notify>,
}

async fn ordered_discovery(
    State(provider): State<OrderedProviderState>,
) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "authorization_endpoint": format!("{}/authorize", provider.base),
        "token_endpoint": format!("{}/token", provider.base),
        "introspection_endpoint": format!("{}/introspect", provider.base),
    }))
}

async fn ordered_introspection(State(provider): State<OrderedProviderState>) -> Response {
    let call = provider.calls.fetch_add(1, Ordering::SeqCst);
    if call == 0 {
        provider.first_started.notify_one();
        provider.release_first.notified().await;
        if !provider.first_active {
            return Json(serde_json::json!({
                "active": false,
                "permissions": [],
            }))
            .into_response();
        }
        return Json(serde_json::json!({
            "active": true,
            "permissions": ["cma:admin"],
        }))
        .into_response();
    }
    Json(serde_json::json!({
        "active": true,
        "permissions": ["cma:session:use"],
    }))
    .into_response()
}

async fn ordered_refresh(State(provider): State<OrderedProviderState>) -> Response {
    provider.refresh_started.notify_one();
    provider.release_refresh.notified().await;
    Json(serde_json::json!({
        "access_token": "access-new",
        "refresh_token": "refresh-new",
        "expires_in": 3600,
    }))
    .into_response()
}

async fn ordered_provider() -> (OrderedProviderState, tokio::task::JoinHandle<()>) {
    ordered_provider_with_first_active(true).await
}

async fn ordered_provider_with_first_active(
    first_active: bool,
) -> (OrderedProviderState, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ordered provider");
    let base = format!(
        "http://{}",
        listener.local_addr().expect("provider address")
    );
    let state = OrderedProviderState {
        base,
        calls: Arc::new(AtomicUsize::new(0)),
        first_active,
        first_started: Arc::new(Notify::new()),
        release_first: Arc::new(Notify::new()),
        refresh_started: Arc::new(Notify::new()),
        release_refresh: Arc::new(Notify::new()),
    };
    let app = Router::new()
        .route("/.well-known/openid-configuration", get(ordered_discovery))
        .route("/introspect", post(ordered_introspection))
        .route("/token", post(ordered_refresh))
        .with_state(state.clone());
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serve ordered provider");
    });
    (state, server)
}

fn provider_auth(base: String, store: Arc<MemoryAuthStore>) -> Arc<AuthState> {
    AuthState::with_store(
        AuthMode::Provider {
            base_url: base,
            client_id: "client".into(),
            client_secret: "secret".into(),
            public_url: "http://service.invalid".into(),
        },
        None,
        store,
    )
}

async fn seed_subject(
    store: &MemoryAuthStore,
    subject: &str,
    access_token: &str,
    refresh_token: Option<&str>,
    expires_at: SystemTime,
) {
    store
        .insert_subject(SubjectAuthState {
            subject: SubjectId(subject.into()),
            incarnation: format!("incarnation-{subject}"),
            revision: 0,
            token_generation: 1,
            tokens: Some(DurableSubjectTokens {
                access_token: Some(SecretValue::new(access_token.into())),
                refresh_token: refresh_token.map(|value| SecretValue::new(value.into())),
                expires_at,
            }),
            refresh_lease: None,
            authorization: Authorization::from_capabilities([Capability::ReadAllSessions]),
            authorization_check_after: SystemTime::UNIX_EPOCH,
            authorization_lease: None,
            profile: super::profile::Profile::default(),
        })
        .await
        .expect("seed subject store")
        .then_some(())
        .expect("seed subject insert");
}

async fn issue_cookie(store: &MemoryAuthStore, subject: &str, cookie: &str) {
    store
        .put_browser_session(BrowserSession {
            id: SecretDigest::identify("browser-session", cookie),
            subject: SubjectId(subject.into()),
            expires_at: SystemTime::now() + Duration::from_secs(600),
        })
        .await
        .expect("browser session persists");
}

async fn expire_authorization_lease(store: &MemoryAuthStore, subject: &str) {
    let subject = SubjectId(subject.into());
    let mut leased = store.subject(&subject).await.unwrap().unwrap();
    let revision = leased.revision;
    leased.revision += 1;
    leased
        .authorization_lease
        .as_mut()
        .expect("authorization check was durably claimed")
        .expires_at = SystemTime::UNIX_EPOCH;
    let incarnation = leased.incarnation.clone();
    assert!(store
        .replace_subject(&incarnation, revision, leased)
        .await
        .unwrap());
}

async fn privileged(Extension(authorization): Extension<Authorization>) -> StatusCode {
    if authorization.allows(Capability::ReadAllSessions) {
        StatusCode::NO_CONTENT
    } else {
        StatusCode::FORBIDDEN
    }
}

async fn cookie_status(auth: Arc<AuthState>, cookie: &str) -> StatusCode {
    Router::new()
        .route("/protected", get(privileged))
        .layer(axum::middleware::from_fn_with_state(
            auth,
            super::attach_subject,
        ))
        .oneshot(
            Request::get("/protected")
                .header(header::COOKIE, format!("{SESSION_COOKIE}={cookie}"))
                .body(Body::empty())
                .expect("cookie request"),
        )
        .await
        .expect("middleware response")
        .status()
}

#[tokio::test]
async fn a_far_from_expiry_token_loses_revoked_capability_on_the_next_cookie_request() {
    let (provider, server) = provider(
        StatusCode::OK,
        StatusCode::OK,
        IntrospectionBody::Active(true),
        &["cma:session:use"],
        None,
    )
    .await;
    let store = Arc::new(MemoryAuthStore::default());
    let auth = provider_auth(provider.base.clone(), store.clone());
    seed_subject(
        &store,
        "subject-a",
        "token:subject-a",
        Some("refresh-a"),
        SystemTime::now() + Duration::from_secs(3600),
    )
    .await;
    issue_cookie(&store, "subject-a", "cookie-a").await;
    assert_eq!(
        cookie_status(auth.clone(), "cookie-a").await,
        StatusCode::FORBIDDEN
    );
    let fact = store
        .subject(&SubjectId("subject-a".into()))
        .await
        .unwrap()
        .unwrap();
    assert!(
        fact.tokens.is_some(),
        "grant reduction does not discard tokens"
    );
    assert!(!fact.authorization.allows(Capability::ReadAllSessions));
    server.abort();
}

#[tokio::test]
async fn transient_userinfo_and_introspection_failures_preserve_the_complete_answer() {
    let cases = [
        (
            StatusCode::SERVICE_UNAVAILABLE,
            StatusCode::SERVICE_UNAVAILABLE,
            IntrospectionBody::Active(true),
            None,
        ),
        (
            StatusCode::SERVICE_UNAVAILABLE,
            StatusCode::SERVICE_UNAVAILABLE,
            IntrospectionBody::MissingActive,
            None,
        ),
        (
            StatusCode::SERVICE_UNAVAILABLE,
            StatusCode::OK,
            IntrospectionBody::MissingActive,
            None,
        ),
        (
            StatusCode::SERVICE_UNAVAILABLE,
            StatusCode::OK,
            IntrospectionBody::MissingPermissions,
            None,
        ),
        (
            StatusCode::SERVICE_UNAVAILABLE,
            StatusCode::OK,
            IntrospectionBody::MalformedPermissions,
            None,
        ),
        (
            StatusCode::SERVICE_UNAVAILABLE,
            StatusCode::OK,
            IntrospectionBody::Malformed,
            None,
        ),
        (
            StatusCode::OK,
            StatusCode::OK,
            IntrospectionBody::MissingActive,
            Some(""),
        ),
    ];
    for (userinfo_status, introspection_status, body, userinfo_subject) in cases {
        let (provider, server) = provider(
            userinfo_status,
            introspection_status,
            body,
            &["cma:session:use"],
            userinfo_subject,
        )
        .await;
        let store = Arc::new(MemoryAuthStore::default());
        let auth = provider_auth(provider.base, store.clone());
        seed_subject(
            &store,
            "subject-a",
            "token:subject-a",
            Some("refresh-a"),
            SystemTime::now() + Duration::from_secs(3600),
        )
        .await;
        crate::renewal::sweep_once(&auth, &super::empty_session_service()).await;

        let fact = store
            .subject(&SubjectId("subject-a".into()))
            .await
            .unwrap()
            .unwrap();
        assert!(fact.authorization.allows(Capability::ReadAllSessions));
        assert_eq!(
            fact.tokens.unwrap().refresh_token.unwrap().expose(),
            "refresh-a"
        );
        assert_eq!(fact.authorization_check_after, SystemTime::UNIX_EPOCH);
        assert!(fact.authorization_lease.is_none());
        server.abort();
    }
}

#[tokio::test]
async fn userinfo_401_and_403_cannot_mask_reduced_or_inactive_introspection() {
    for userinfo_status in [StatusCode::UNAUTHORIZED, StatusCode::FORBIDDEN] {
        for active in [true, false] {
            let (provider, server) = provider(
                userinfo_status,
                StatusCode::OK,
                IntrospectionBody::Active(active),
                &["cma:session:use"],
                None,
            )
            .await;
            let store = Arc::new(MemoryAuthStore::default());
            let auth = provider_auth(provider.base.clone(), store.clone());
            seed_subject(
                &store,
                "subject-a",
                "token:subject-a",
                Some("refresh-a"),
                SystemTime::now() + Duration::from_secs(3600),
            )
            .await;

            crate::renewal::sweep_once(&auth, &super::empty_session_service()).await;

            let fact = store
                .subject(&SubjectId("subject-a".into()))
                .await
                .unwrap()
                .unwrap();
            assert!(!fact.authorization.allows(Capability::ReadAllSessions));
            let tokens = fact.tokens.expect("the refresh grant remains eligible");
            assert_eq!(tokens.access_token.is_some(), active);
            assert_eq!(tokens.refresh_token.unwrap().expose(), "refresh-a");
            assert_eq!(provider.introspection_calls.load(Ordering::SeqCst), 1);
            assert!(provider.seen_userinfo_tokens.lock().unwrap().is_empty());
            server.abort();
        }
    }
}

#[tokio::test]
async fn two_replicas_share_one_durable_authorization_check() {
    let (provider, server) = provider(
        StatusCode::OK,
        StatusCode::OK,
        IntrospectionBody::Active(true),
        &["cma:session:use"],
        None,
    )
    .await;
    let store = Arc::new(MemoryAuthStore::default());
    let first = provider_auth(provider.base.clone(), store.clone());
    let second = provider_auth(provider.base.clone(), store.clone());
    seed_subject(
        &store,
        "subject-a",
        "token:subject-a",
        Some("refresh-a"),
        SystemTime::now() + Duration::from_secs(3600),
    )
    .await;
    let selected = store
        .subject(&SubjectId("subject-a".into()))
        .await
        .unwrap()
        .unwrap();
    let first_budget = AuthOperationBudget::standalone(AUTHORIZATION_CHECK_ATTEMPT_BUDGET);
    let second_budget = AuthOperationBudget::standalone(AUTHORIZATION_CHECK_ATTEMPT_BUDGET);

    let (left, right) = tokio::join!(
        first.reconcile_authorization_with_budget(
            selected.clone(),
            SystemTime::now(),
            &first_budget,
        ),
        second.reconcile_authorization_with_budget(selected, SystemTime::now(), &second_budget,),
    );

    let outcomes = [left.unwrap(), right.unwrap()];
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| matches!(outcome, AuthorizationCheckOutcome::Replaced))
            .count(),
        1
    );
    assert_eq!(provider.introspection_calls.load(Ordering::SeqCst), 1);
    assert_eq!(provider.seen_introspection_tokens.lock().unwrap().len(), 1);
    assert!(provider.seen_userinfo_tokens.lock().unwrap().is_empty());
    let durable = store
        .subject(&SubjectId("subject-a".into()))
        .await
        .unwrap()
        .unwrap();
    assert!(durable.authorization_lease.is_none());
    assert!(!durable.authorization.allows(Capability::ReadAllSessions));
    server.abort();
}

#[tokio::test]
async fn an_expired_lease_takeover_fences_a_late_more_privileged_answer() {
    let (provider, server) = ordered_provider().await;
    let store = Arc::new(MemoryAuthStore::default());
    let first = provider_auth(provider.base.clone(), store.clone());
    let second = provider_auth(provider.base.clone(), store.clone());
    seed_subject(
        &store,
        "subject-a",
        "token:subject-a",
        Some("refresh-a"),
        SystemTime::now() + Duration::from_secs(3600),
    )
    .await;
    let selected = store
        .subject(&SubjectId("subject-a".into()))
        .await
        .unwrap()
        .unwrap();
    let first_task = tokio::spawn(async move {
        let budget = AuthOperationBudget::standalone(AUTHORIZATION_CHECK_ATTEMPT_BUDGET);
        first
            .reconcile_authorization_with_budget(selected, SystemTime::now(), &budget)
            .await
    });
    provider.first_started.notified().await;
    expire_authorization_lease(&store, "subject-a").await;

    let takeover = store
        .subject(&SubjectId("subject-a".into()))
        .await
        .unwrap()
        .unwrap();
    let takeover_budget = AuthOperationBudget::standalone(AUTHORIZATION_CHECK_ATTEMPT_BUDGET);
    let takeover_outcome = second
        .reconcile_authorization_with_budget(takeover, SystemTime::now(), &takeover_budget)
        .await
        .unwrap();
    assert!(matches!(
        takeover_outcome,
        AuthorizationCheckOutcome::Replaced
    ));

    provider.release_first.notify_one();
    let late_outcome = first_task.await.unwrap().unwrap();
    assert!(matches!(late_outcome, AuthorizationCheckOutcome::Skipped));
    let durable = store
        .subject(&SubjectId("subject-a".into()))
        .await
        .unwrap()
        .unwrap();
    assert!(!durable.authorization.allows(Capability::ReadAllSessions));
    assert!(durable.authorization_lease.is_none());
    assert_eq!(provider.calls.load(Ordering::SeqCst), 2);
    server.abort();
}

#[tokio::test]
async fn refresh_rotation_cancels_an_inflight_check_before_it_can_quarantine_the_old_token() {
    let (provider, server) = ordered_provider_with_first_active(false).await;
    let store = Arc::new(MemoryAuthStore::default());
    let checking = provider_auth(provider.base.clone(), store.clone());
    let refreshing = provider_auth(provider.base.clone(), store.clone());
    seed_subject(
        &store,
        "subject-a",
        "access-old",
        Some("refresh-old"),
        SystemTime::now() + Duration::from_secs(60),
    )
    .await;
    let selected = store
        .subject(&SubjectId("subject-a".into()))
        .await
        .unwrap()
        .unwrap();
    let incarnation = selected.incarnation.clone();
    let token_generation = selected.token_generation;
    let check = tokio::spawn(async move {
        let budget = AuthOperationBudget::standalone(AUTHORIZATION_CHECK_ATTEMPT_BUDGET);
        checking
            .reconcile_authorization_with_budget(selected, SystemTime::now(), &budget)
            .await
    });
    provider.first_started.notified().await;

    let subject = SubjectId("subject-a".into());
    let refresh = tokio::spawn(async move {
        refreshing
            .refresh(&subject, "refresh-old", &incarnation, token_generation)
            .await
    });
    provider.refresh_started.notified().await;

    // The provider has consumed the rotating refresh token, but withholds the
    // response while the old access token is reported inactive.
    provider.release_first.notify_one();
    assert!(matches!(
        check.await.unwrap().unwrap(),
        AuthorizationCheckOutcome::Skipped
    ));
    provider.release_refresh.notify_one();
    assert_eq!(refresh.await.unwrap().unwrap(), "access-new");

    let durable = store
        .subject(&SubjectId("subject-a".into()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(durable.token_generation, token_generation + 1);
    assert!(durable.authorization_lease.is_none());
    assert!(durable.refresh_lease.is_none());
    assert_eq!(durable.authorization_check_after, SystemTime::UNIX_EPOCH);
    let tokens = durable.tokens.unwrap();
    assert_eq!(tokens.access_token.unwrap().expose(), "access-new");
    assert_eq!(tokens.refresh_token.unwrap().expose(), "refresh-new");
    server.abort();
}

#[tokio::test]
async fn a_cancelled_worker_is_taken_over_after_its_durable_lease_expires() {
    let (provider, server) = ordered_provider().await;
    let store = Arc::new(MemoryAuthStore::default());
    let first = provider_auth(provider.base.clone(), store.clone());
    let restarted = provider_auth(provider.base.clone(), store.clone());
    seed_subject(
        &store,
        "subject-a",
        "token:subject-a",
        Some("refresh-a"),
        SystemTime::now() + Duration::from_secs(3600),
    )
    .await;
    let selected = store
        .subject(&SubjectId("subject-a".into()))
        .await
        .unwrap()
        .unwrap();
    let cancelled = tokio::spawn(async move {
        let budget = AuthOperationBudget::standalone(AUTHORIZATION_CHECK_ATTEMPT_BUDGET);
        first
            .reconcile_authorization_with_budget(selected, SystemTime::now(), &budget)
            .await
    });
    provider.first_started.notified().await;
    cancelled.abort();
    let _ = cancelled.await;

    let still_leased = store
        .subject(&SubjectId("subject-a".into()))
        .await
        .unwrap()
        .unwrap();
    assert!(still_leased.authorization_lease.is_some());
    let blocked_budget = AuthOperationBudget::standalone(AUTHORIZATION_CHECK_ATTEMPT_BUDGET);
    let blocked = restarted
        .reconcile_authorization_with_budget(still_leased, SystemTime::now(), &blocked_budget)
        .await
        .unwrap();
    assert!(matches!(blocked, AuthorizationCheckOutcome::Skipped));
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);

    expire_authorization_lease(&store, "subject-a").await;
    let expired = store
        .subject(&SubjectId("subject-a".into()))
        .await
        .unwrap()
        .unwrap();
    let takeover_budget = AuthOperationBudget::standalone(AUTHORIZATION_CHECK_ATTEMPT_BUDGET);
    let takeover = restarted
        .reconcile_authorization_with_budget(expired, SystemTime::now(), &takeover_budget)
        .await
        .unwrap();
    assert!(matches!(takeover, AuthorizationCheckOutcome::Replaced));
    assert_eq!(provider.calls.load(Ordering::SeqCst), 2);
    provider.release_first.notify_one();
    server.abort();
}

#[tokio::test]
async fn an_overdue_cookie_fails_closed_within_the_request_allowance_for_a_slow_provider() {
    let (provider, server) = ordered_provider().await;
    let store = Arc::new(MemoryAuthStore::default());
    let auth = provider_auth(provider.base.clone(), store.clone());
    seed_subject(
        &store,
        "subject-a",
        "token:subject-a",
        Some("refresh-a"),
        SystemTime::now() + Duration::from_secs(3600),
    )
    .await;
    issue_cookie(&store, "subject-a", "slow-cookie").await;

    let started = Instant::now();
    let status = cookie_status(auth, "slow-cookie").await;

    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(started.elapsed() < Duration::from_secs(1));
    let durable = store
        .subject(&SubjectId("subject-a".into()))
        .await
        .unwrap()
        .unwrap();
    assert!(durable.authorization_lease.is_some());
    assert!(durable.authorization.allows(Capability::ReadAllSessions));
    provider.release_first.notify_one();
    server.abort();
}

#[tokio::test]
async fn a_successful_check_schedules_from_provider_completion_not_sweep_start() {
    let (provider, server) = ordered_provider().await;
    let store = Arc::new(MemoryAuthStore::default());
    let auth = provider_auth(provider.base.clone(), store.clone());
    seed_subject(
        &store,
        "subject-a",
        "token:subject-a",
        Some("refresh-a"),
        SystemTime::now() + Duration::from_secs(3600),
    )
    .await;
    let selected = store
        .subject(&SubjectId("subject-a".into()))
        .await
        .unwrap()
        .unwrap();
    let task = tokio::spawn(async move {
        let budget = AuthOperationBudget::standalone(AUTHORIZATION_CHECK_ATTEMPT_BUDGET);
        auth.reconcile_authorization_with_budget(selected, SystemTime::now(), &budget)
            .await
    });
    provider.first_started.notified().await;
    tokio::time::sleep(Duration::from_millis(25)).await;
    let completion_floor = SystemTime::now();
    provider.release_first.notify_one();
    assert!(matches!(
        task.await.unwrap().unwrap(),
        AuthorizationCheckOutcome::Replaced
    ));

    let durable = store
        .subject(&SubjectId("subject-a".into()))
        .await
        .unwrap()
        .unwrap();
    assert!(
        durable.authorization_check_after >= completion_floor + super::AUTHORIZATION_CHECK_INTERVAL
    );
    server.abort();
}

#[tokio::test]
async fn a_complete_answer_never_extends_authority_past_access_token_expiry() {
    let (provider, server) = provider(
        StatusCode::OK,
        StatusCode::OK,
        IntrospectionBody::Active(true),
        &["cma:session:use"],
        None,
    )
    .await;
    let store = Arc::new(MemoryAuthStore::default());
    let auth = provider_auth(provider.base, store.clone());
    let expires_at = SystemTime::now() + Duration::from_secs(30);
    seed_subject(&store, "subject-a", "access-only", None, expires_at).await;

    crate::renewal::sweep_once(&auth, &super::empty_session_service()).await;

    let durable = store
        .subject(&SubjectId("subject-a".into()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(durable.authorization_check_after, expires_at);
    assert!(!durable.authorization.allows(Capability::ReadAllSessions));
    server.abort();
}

#[tokio::test]
async fn a_transient_answer_releases_its_claim_and_remains_immediately_due() {
    let (unavailable, unavailable_server) = provider(
        StatusCode::SERVICE_UNAVAILABLE,
        StatusCode::SERVICE_UNAVAILABLE,
        IntrospectionBody::Active(true),
        &[],
        None,
    )
    .await;
    let store = Arc::new(MemoryAuthStore::default());
    let first_auth = provider_auth(unavailable.base, store.clone());
    seed_subject(
        &store,
        "subject-a",
        "token:subject-a",
        Some("refresh-a"),
        SystemTime::now() + Duration::from_secs(3600),
    )
    .await;
    let selected = store
        .subject(&SubjectId("subject-a".into()))
        .await
        .unwrap()
        .unwrap();
    let due_at = SystemTime::now();
    let budget = AuthOperationBudget::standalone(AUTHORIZATION_CHECK_ATTEMPT_BUDGET);
    let outcome = first_auth
        .reconcile_authorization_with_budget(selected, due_at, &budget)
        .await
        .unwrap();
    assert!(matches!(outcome, AuthorizationCheckOutcome::Preserved(_)));
    unavailable_server.abort();

    let retry_at = SystemTime::now();
    let retry = store
        .subjects_due_for_authorization_check(retry_at, None, 1)
        .await
        .unwrap()
        .pop()
        .expect("transient subject remains immediately due")
        .state
        .unwrap();

    let (healthy, healthy_server) = provider(
        StatusCode::OK,
        StatusCode::OK,
        IntrospectionBody::Active(true),
        &["cma:session:use"],
        None,
    )
    .await;
    let retry_auth = provider_auth(healthy.base, store.clone());
    let retry_budget = AuthOperationBudget::standalone(AUTHORIZATION_CHECK_ATTEMPT_BUDGET);
    retry_auth
        .reconcile_authorization_with_budget(retry, retry_at, &retry_budget)
        .await
        .unwrap();
    assert!(!store
        .subject(&SubjectId("subject-a".into()))
        .await
        .unwrap()
        .unwrap()
        .authorization
        .allows(Capability::ReadAllSessions));
    healthy_server.abort();
}

#[tokio::test]
async fn authoritative_inactive_and_subject_mismatch_answers_fail_closed() {
    let (inactive, inactive_server) = provider(
        StatusCode::OK,
        StatusCode::OK,
        IntrospectionBody::Active(false),
        &[],
        None,
    )
    .await;
    let inactive_store = Arc::new(MemoryAuthStore::default());
    let inactive_auth = provider_auth(inactive.base, inactive_store.clone());
    seed_subject(
        &inactive_store,
        "subject-a",
        "token:subject-a",
        Some("refresh-a"),
        SystemTime::now() + Duration::from_secs(3600),
    )
    .await;
    issue_cookie(&inactive_store, "subject-a", "inactive-cookie").await;
    crate::renewal::sweep_once(&inactive_auth, &super::empty_session_service()).await;
    let inactive_fact = inactive_store
        .subject(&SubjectId("subject-a".into()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(inactive_fact.authorization.capabilities().count(), 0);
    let inactive_tokens = inactive_fact
        .tokens
        .expect("inactive access introspection does not invalidate the refresh grant");
    assert!(inactive_tokens.access_token.is_none());
    assert_eq!(inactive_tokens.refresh_token.unwrap().expose(), "refresh-a");
    let through_sequence = inactive_store.auth_sweep_high_water().await.unwrap();
    let refreshable = inactive_store
        .unleased_subjects_due_for_refresh(SystemTime::now(), None, through_sequence, 10)
        .await
        .unwrap();
    assert_eq!(refreshable.len(), 1, "quarantine forces immediate refresh");
    let mut headers = HeaderMap::new();
    headers.insert(
        header::COOKIE,
        format!("{SESSION_COOKIE}=inactive-cookie").parse().unwrap(),
    );
    let resolved = inactive_auth
        .resolve_request(&headers)
        .await
        .unwrap()
        .expect("the cookie still authenticates");
    assert!(resolved.delegated_credential.is_none());
    inactive_server.abort();

    let (mismatch, mismatch_server) = provider(
        StatusCode::OK,
        StatusCode::SERVICE_UNAVAILABLE,
        IntrospectionBody::Active(true),
        &[],
        Some("another-subject"),
    )
    .await;
    let mismatch_store = Arc::new(MemoryAuthStore::default());
    let mismatch_auth = provider_auth(mismatch.base, mismatch_store.clone());
    seed_subject(
        &mismatch_store,
        "subject-a",
        "token:subject-a",
        Some("refresh-a"),
        SystemTime::now() + Duration::from_secs(3600),
    )
    .await;
    crate::renewal::sweep_once(&mismatch_auth, &super::empty_session_service()).await;
    let mismatch_fact = mismatch_store
        .subject(&SubjectId("subject-a".into()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(mismatch_fact.authorization.capabilities().count(), 0);
    assert!(mismatch_fact.tokens.is_none());
    mismatch_server.abort();
}

#[tokio::test]
async fn an_active_introspection_answer_for_another_subject_retires_the_delegation() {
    let (provider, server) = provider(
        StatusCode::SERVICE_UNAVAILABLE,
        StatusCode::OK,
        IntrospectionBody::SubjectMismatch,
        &["cma:admin"],
        None,
    )
    .await;
    let store = Arc::new(MemoryAuthStore::default());
    let auth = provider_auth(provider.base, store.clone());
    seed_subject(
        &store,
        "subject-a",
        "token:subject-a",
        Some("refresh-a"),
        SystemTime::now() + Duration::from_secs(3600),
    )
    .await;

    crate::renewal::sweep_once(&auth, &super::empty_session_service()).await;

    let durable = store
        .subject(&SubjectId("subject-a".into()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(durable.authorization.capabilities().count(), 0);
    assert!(durable.tokens.is_none());
    assert!(provider.seen_userinfo_tokens.lock().unwrap().is_empty());
    server.abort();
}

#[tokio::test]
async fn an_explicit_empty_permission_array_authoritatively_clears_the_grant() {
    let (provider, server) = provider(
        StatusCode::OK,
        StatusCode::OK,
        IntrospectionBody::Active(true),
        &[],
        None,
    )
    .await;
    let store = Arc::new(MemoryAuthStore::default());
    let auth = provider_auth(provider.base, store.clone());
    seed_subject(
        &store,
        "subject-a",
        "token:subject-a",
        Some("refresh-a"),
        SystemTime::now() + Duration::from_secs(3600),
    )
    .await;

    crate::renewal::sweep_once(&auth, &super::empty_session_service()).await;

    let fact = store
        .subject(&SubjectId("subject-a".into()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(fact.authorization.capabilities().count(), 0);
    assert!(fact.tokens.is_some());
    server.abort();
}

#[tokio::test]
async fn a_nonrenewable_expired_delegation_fails_closed_without_provider_authority() {
    let (provider, server) = provider(
        StatusCode::SERVICE_UNAVAILABLE,
        StatusCode::SERVICE_UNAVAILABLE,
        IntrospectionBody::Malformed,
        &[],
        None,
    )
    .await;
    let store = Arc::new(MemoryAuthStore::default());
    let auth = provider_auth(provider.base, store.clone());
    seed_subject(
        &store,
        "subject-a",
        "token:subject-a",
        None,
        SystemTime::now() - Duration::from_secs(1),
    )
    .await;

    crate::renewal::sweep_once(&auth, &super::empty_session_service()).await;

    let fact = store
        .subject(&SubjectId("subject-a".into()))
        .await
        .unwrap()
        .unwrap();
    assert!(fact.tokens.is_none());
    assert_eq!(fact.authorization.capabilities().count(), 0);
    server.abort();
}

#[tokio::test]
async fn browser_sign_in_still_accepts_a_token_without_refresh_custody() {
    let store = Arc::new(MemoryAuthStore::default());
    let auth = provider_auth("http://provider.invalid".into(), store.clone());
    auth.store_sign_in(
        &SubjectId("subject-a".into()),
        DurableSubjectTokens {
            access_token: Some(SecretValue::new("access-only".into())),
            refresh_token: None,
            expires_at: SystemTime::now() + Duration::from_secs(600),
        },
        Authorization::from_capabilities([Capability::ReadAllSessions]),
        super::profile::Profile::default(),
    )
    .await
    .expect("a provider is not required to issue refresh custody at sign-in");
    assert!(store
        .subject(&SubjectId("subject-a".into()))
        .await
        .unwrap()
        .unwrap()
        .tokens
        .unwrap()
        .refresh_token
        .is_none());
}

#[tokio::test]
async fn an_expired_access_only_sign_in_cannot_serve_cookie_authority() {
    let store = Arc::new(MemoryAuthStore::default());
    let auth = provider_auth("http://provider.invalid".into(), store.clone());
    auth.store_sign_in(
        &SubjectId("subject-a".into()),
        DurableSubjectTokens {
            access_token: Some(SecretValue::new("access-expired".into())),
            refresh_token: None,
            expires_at: SystemTime::UNIX_EPOCH,
        },
        Authorization::from_capabilities([Capability::ReadAllSessions]),
        super::profile::Profile::default(),
    )
    .await
    .unwrap();
    issue_cookie(&store, "subject-a", "expired-cookie").await;

    assert_eq!(
        cookie_status(auth, "expired-cookie").await,
        StatusCode::FORBIDDEN
    );
    let durable = store
        .subject(&SubjectId("subject-a".into()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(durable.authorization.capabilities().count(), 0);
    assert!(durable.tokens.is_none());
}

#[tokio::test]
async fn a_tokenless_legacy_grant_cannot_serve_cookie_authority() {
    let store = Arc::new(MemoryAuthStore::default());
    let auth = provider_auth("http://provider.invalid".into(), store.clone());
    let subject = SubjectId("subject-a".into());
    auth.store_authorization(
        &subject,
        Authorization::from_capabilities([Capability::ReadAllSessions]),
    )
    .await
    .unwrap();
    let mut legacy = store.subject(&subject).await.unwrap().unwrap();
    let revision = legacy.revision;
    let incarnation = legacy.incarnation.clone();
    legacy.revision += 1;
    legacy.authorization_check_after = SystemTime::now() + Duration::from_secs(60);
    assert!(store
        .replace_subject(&incarnation, revision, legacy)
        .await
        .unwrap());
    issue_cookie(&store, "subject-a", "tokenless-cookie").await;

    assert_eq!(
        cookie_status(auth, "tokenless-cookie").await,
        StatusCode::SERVICE_UNAVAILABLE
    );
    let durable = store.subject(&subject).await.unwrap().unwrap();
    assert!(durable.authorization.allows(Capability::ReadAllSessions));
    assert!(durable.tokens.is_none());
}

#[tokio::test]
async fn a_legacy_future_deadline_cannot_outlive_its_expired_access_token() {
    let store = Arc::new(MemoryAuthStore::default());
    let auth = provider_auth("http://provider.invalid".into(), store.clone());
    seed_subject(
        &store,
        "subject-a",
        "access-expired",
        None,
        SystemTime::UNIX_EPOCH,
    )
    .await;
    let subject = SubjectId("subject-a".into());
    let mut legacy = store.subject(&subject).await.unwrap().unwrap();
    let revision = legacy.revision;
    let incarnation = legacy.incarnation.clone();
    legacy.revision += 1;
    legacy.authorization_check_after = SystemTime::now() + Duration::from_secs(60);
    assert!(store
        .replace_subject(&incarnation, revision, legacy)
        .await
        .unwrap());
    issue_cookie(&store, "subject-a", "legacy-cookie").await;

    assert_eq!(
        cookie_status(auth, "legacy-cookie").await,
        StatusCode::FORBIDDEN
    );
    let durable = store.subject(&subject).await.unwrap().unwrap();
    assert_eq!(durable.authorization.capabilities().count(), 0);
    assert!(durable.tokens.is_none());
}

#[tokio::test]
async fn a_cas_miss_preserves_a_concurrently_rotated_token_and_refresh_lease() {
    let (provider, server) = provider(
        StatusCode::OK,
        StatusCode::OK,
        IntrospectionBody::Active(true),
        &["cma:session:use"],
        None,
    )
    .await;
    let store = Arc::new(MemoryAuthStore::default());
    let auth = provider_auth(provider.base, store.clone());
    seed_subject(
        &store,
        "subject-a",
        "token:subject-a:old",
        Some("refresh-old"),
        SystemTime::now() + Duration::from_secs(3600),
    )
    .await;
    let selected = store
        .subject(&SubjectId("subject-a".into()))
        .await
        .unwrap()
        .unwrap();
    let mut winner = selected.clone();
    winner.revision += 1;
    winner.token_generation += 1;
    winner.tokens = Some(DurableSubjectTokens {
        access_token: Some(SecretValue::new("token:subject-a:new".into())),
        refresh_token: Some(SecretValue::new("refresh-new".into())),
        expires_at: SystemTime::now() + Duration::from_secs(7200),
    });
    winner.refresh_lease = Some(SubjectRefreshLease {
        holder: "refresh-winner".into(),
        expires_at: SystemTime::now() + Duration::from_secs(45),
    });
    store.install_subject_winner_on_replace(1, winner).await;
    let budget = AuthOperationBudget::standalone(AUTHORIZATION_CHECK_ATTEMPT_BUDGET);

    let outcome = auth
        .reconcile_authorization_with_budget(selected, SystemTime::now(), &budget)
        .await
        .unwrap();

    assert!(matches!(outcome, AuthorizationCheckOutcome::Skipped));
    let preserved = store
        .subject(&SubjectId("subject-a".into()))
        .await
        .unwrap()
        .unwrap();
    let tokens = preserved.tokens.unwrap();
    assert_eq!(tokens.access_token.unwrap().expose(), "token:subject-a:new");
    assert_eq!(tokens.refresh_token.unwrap().expose(), "refresh-new");
    assert_eq!(preserved.refresh_lease.unwrap().holder, "refresh-winner");
    assert_eq!(
        preserved.authorization_check_after,
        SystemTime::UNIX_EPOCH,
        "lease contention leaves the authorization row due for the next sweep"
    );
    server.abort();
}

#[tokio::test]
async fn a_refresh_lease_expiring_at_the_observation_boundary_does_not_block_reconciliation() {
    let (provider, server) = provider(
        StatusCode::OK,
        StatusCode::OK,
        IntrospectionBody::Active(true),
        &["cma:session:use"],
        None,
    )
    .await;
    let store = Arc::new(MemoryAuthStore::default());
    let auth = provider_auth(provider.base, store.clone());
    seed_subject(
        &store,
        "subject-a",
        "token:subject-a",
        Some("refresh-a"),
        SystemTime::now() + Duration::from_secs(3600),
    )
    .await;
    let due_at = SystemTime::now();
    let mut selected = store
        .subject(&SubjectId("subject-a".into()))
        .await
        .unwrap()
        .unwrap();
    let revision = selected.revision;
    selected.revision += 1;
    selected.refresh_lease = Some(SubjectRefreshLease {
        holder: "expired-worker".into(),
        expires_at: due_at,
    });
    assert!(store
        .replace_subject(&selected.incarnation, revision, selected.clone())
        .await
        .unwrap());
    let budget = AuthOperationBudget::standalone(AUTHORIZATION_CHECK_ATTEMPT_BUDGET);

    let outcome = auth
        .reconcile_authorization_with_budget(selected, due_at, &budget)
        .await
        .unwrap();

    assert!(matches!(outcome, AuthorizationCheckOutcome::Replaced));
    let durable = store
        .subject(&SubjectId("subject-a".into()))
        .await
        .unwrap()
        .unwrap();
    assert!(!durable.authorization.allows(Capability::ReadAllSessions));
    assert_eq!(durable.refresh_lease.unwrap().expires_at, due_at);
    server.abort();
}

#[tokio::test]
async fn a_rotated_token_generation_is_claimed_before_any_provider_request() {
    let (provider, server) = provider(
        StatusCode::OK,
        StatusCode::OK,
        IntrospectionBody::Active(true),
        &["cma:session:use"],
        None,
    )
    .await;
    let store = Arc::new(MemoryAuthStore::default());
    let auth = provider_auth(provider.base, store.clone());
    seed_subject(
        &store,
        "subject-a",
        "token:subject-a:old",
        Some("refresh-old"),
        SystemTime::now() + Duration::from_secs(3600),
    )
    .await;
    let selected = store
        .subject(&SubjectId("subject-a".into()))
        .await
        .unwrap()
        .unwrap();
    let mut winner = selected.clone();
    winner.revision += 1;
    winner.token_generation += 1;
    winner.tokens = Some(DurableSubjectTokens {
        access_token: Some(SecretValue::new("token:subject-a:new".into())),
        refresh_token: Some(SecretValue::new("refresh-new".into())),
        expires_at: SystemTime::now() + Duration::from_secs(7200),
    });
    store.install_subject_winner_on_replace(1, winner).await;
    let budget = AuthOperationBudget::standalone(AUTHORIZATION_CHECK_ATTEMPT_BUDGET);

    auth.reconcile_authorization_with_budget(selected, SystemTime::now(), &budget)
        .await
        .unwrap();

    assert_eq!(
        *provider.seen_introspection_tokens.lock().unwrap(),
        vec!["token:subject-a:new".to_string()]
    );
    let preserved = store
        .subject(&SubjectId("subject-a".into()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        preserved.tokens.unwrap().refresh_token.unwrap().expose(),
        "refresh-new"
    );
    server.abort();
}

#[tokio::test]
async fn a_recreated_subject_incarnation_is_claimed_before_any_provider_request() {
    let (provider, server) = provider(
        StatusCode::OK,
        StatusCode::OK,
        IntrospectionBody::Active(true),
        &["cma:session:use"],
        None,
    )
    .await;
    let store = Arc::new(MemoryAuthStore::default());
    let auth = provider_auth(provider.base.clone(), store.clone());
    seed_subject(
        &store,
        "subject-a",
        "token:subject-a:old",
        Some("refresh-old"),
        SystemTime::now() + Duration::from_secs(3600),
    )
    .await;
    let selected = store
        .subject(&SubjectId("subject-a".into()))
        .await
        .unwrap()
        .unwrap();
    let mut recreated = selected.clone();
    recreated.incarnation = "recreated-incarnation".into();
    recreated.revision = 0;
    recreated.tokens = Some(DurableSubjectTokens {
        access_token: Some(SecretValue::new("token:subject-a:new".into())),
        refresh_token: Some(SecretValue::new("refresh-new".into())),
        expires_at: SystemTime::now() + Duration::from_secs(7200),
    });
    store.install_subject_winner_on_replace(1, recreated).await;
    let budget = AuthOperationBudget::standalone(AUTHORIZATION_CHECK_ATTEMPT_BUDGET);

    auth.reconcile_authorization_with_budget(selected, SystemTime::now(), &budget)
        .await
        .unwrap();

    assert_eq!(
        *provider.seen_introspection_tokens.lock().unwrap(),
        vec!["token:subject-a:new".to_string()]
    );
    let durable = store
        .subject(&SubjectId("subject-a".into()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(durable.incarnation, "recreated-incarnation");
    assert_eq!(durable.revision, 2);
    assert_eq!(durable.token_generation, 1);
    assert_eq!(
        durable.tokens.unwrap().refresh_token.unwrap().expose(),
        "refresh-new"
    );
    server.abort();
}

#[tokio::test]
async fn repeated_cas_contention_stops_at_the_explicit_budget_and_stays_due() {
    let (provider, server) = provider(
        StatusCode::OK,
        StatusCode::OK,
        IntrospectionBody::Active(true),
        &["cma:session:use"],
        None,
    )
    .await;
    let store = Arc::new(MemoryAuthStore::default());
    let auth = provider_auth(provider.base, store.clone());
    seed_subject(
        &store,
        "subject-a",
        "token:subject-a",
        Some("refresh-a"),
        SystemTime::now() + Duration::from_secs(3600),
    )
    .await;
    let selected = store
        .subject(&SubjectId("subject-a".into()))
        .await
        .unwrap()
        .unwrap();
    store.contend_subject_replaces(true);
    let budget = AuthOperationBudget::standalone(AUTHORIZATION_CHECK_ATTEMPT_BUDGET);

    assert!(auth
        .reconcile_authorization_with_budget(selected, SystemTime::now(), &budget,)
        .await
        .is_err());

    assert_eq!(store.subject_replace_calls(), 4);
    let fact = store
        .subject(&SubjectId("subject-a".into()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(fact.authorization_check_after, SystemTime::UNIX_EPOCH);
    assert!(fact.authorization.allows(Capability::ReadAllSessions));
    server.abort();
}

#[tokio::test]
async fn an_overdue_cookie_bypasses_a_multipage_backlog_and_the_sweep_keeps_paging() {
    let (provider, server) = provider(
        StatusCode::OK,
        StatusCode::OK,
        IntrospectionBody::Active(true),
        &["cma:session:use"],
        None,
    )
    .await;
    let store = Arc::new(MemoryAuthStore::default());
    let auth = provider_auth(provider.base, store.clone());
    for number in 0..(super::AUTH_SWEEP_BATCH_SIZE + 2) {
        let subject = format!("subject-{number:03}");
        seed_subject(
            &store,
            &subject,
            &format!("token:{subject}"),
            Some("refresh"),
            SystemTime::now() + Duration::from_secs(3600),
        )
        .await;
    }
    issue_cookie(&store, "subject-101", "backlog-cookie").await;
    assert_eq!(
        cookie_status(auth.clone(), "backlog-cookie").await,
        StatusCode::FORBIDDEN,
        "request-time enforcement does not wait behind the sweep backlog"
    );
    store
        .fail_subject_read(Some(SubjectId("subject-099".into())))
        .await;

    crate::renewal::sweep_once(&auth, &super::empty_session_service()).await;
    assert_eq!(
        provider.seen_introspection_tokens.lock().unwrap().len(),
        1 + super::AUTHORIZATION_CHECK_BATCH_SIZE,
        "one slow-worker pass admits only one provider batch"
    );
    for _ in 1..7 {
        crate::renewal::sweep_once(&auth, &super::empty_session_service()).await;
    }

    store.fail_subject_read(None).await;
    let failed = store
        .subject(&SubjectId("subject-099".into()))
        .await
        .unwrap()
        .unwrap();
    assert!(failed.authorization.allows(Capability::ReadAllSessions));
    assert_eq!(failed.authorization_check_after, SystemTime::UNIX_EPOCH);
    let last = store
        .subject(&SubjectId("subject-101".into()))
        .await
        .unwrap()
        .unwrap();
    assert!(!last.authorization.allows(Capability::ReadAllSessions));
    assert_eq!(
        provider.seen_introspection_tokens.lock().unwrap().len(),
        101
    );
    server.abort();
}
