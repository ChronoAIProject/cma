use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use axum::body::Body;
use axum::extract::{Extension, State};
use axum::http::{header, HeaderMap, Request, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use layer2_core::{Authorization, Capability, SubjectId};
use tower::ServiceExt;

use super::persistence::{
    AuthStore, BrowserSession, DurableSubjectTokens, MemoryAuthStore, SecretDigest, SecretValue,
    SubjectAuthState, SubjectRefreshLease,
};
use super::{
    AuthMode, AuthOperationBudget, AuthState, AuthorizationCheckOutcome, AUTHORIZATION_CHECK_ATTEMPT_BUDGET,
    SESSION_COOKIE,
};

#[derive(Clone)]
enum IntrospectionBody {
    Active(bool),
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

async fn introspection(State(provider): State<ProviderState>) -> Response {
    if !provider.introspection_status.is_success() {
        return provider.introspection_status.into_response();
    }
    match provider.introspection_body {
        IntrospectionBody::Active(active) => Json(serde_json::json!({
            "active": active,
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
    let base = format!("http://{}", listener.local_addr().expect("provider address"));
    let state = ProviderState {
        base,
        userinfo_status,
        userinfo_subject: userinfo_subject.map(str::to_string),
        introspection_status,
        introspection_body,
        permissions: permissions.iter().map(|value| (*value).to_string()).collect(),
        seen_userinfo_tokens: Arc::new(Mutex::new(Vec::new())),
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
                access_token: SecretValue::new(access_token.into()),
                refresh_token: refresh_token.map(|value| SecretValue::new(value.into())),
                expires_at,
            }),
            refresh_lease: None,
            authorization: Authorization::from_capabilities([Capability::ReadAllSessions]),
            authorization_check_after: SystemTime::UNIX_EPOCH,
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
        StatusCode::NO_CONTENT
    );

    crate::renewal::sweep_once(&auth, &super::empty_session_service()).await;

    assert_eq!(
        cookie_status(auth, "cookie-a").await,
        StatusCode::FORBIDDEN
    );
    let fact = store
        .subject(&SubjectId("subject-a".into()))
        .await
        .unwrap()
        .unwrap();
    assert!(fact.tokens.is_some(), "grant reduction does not discard tokens");
    assert!(!fact.authorization.allows(Capability::ReadAllSessions));
    server.abort();
}

#[tokio::test]
async fn transient_userinfo_and_introspection_failures_preserve_the_complete_answer() {
    let cases = [
        (
            StatusCode::FORBIDDEN,
            StatusCode::OK,
            IntrospectionBody::Active(true),
            None,
        ),
        (
            StatusCode::OK,
            StatusCode::SERVICE_UNAVAILABLE,
            IntrospectionBody::Active(true),
            None,
        ),
        (
            StatusCode::OK,
            StatusCode::FORBIDDEN,
            IntrospectionBody::Active(true),
            None,
        ),
        (
            StatusCode::OK,
            StatusCode::OK,
            IntrospectionBody::MissingActive,
            None,
        ),
        (
            StatusCode::OK,
            StatusCode::OK,
            IntrospectionBody::MissingPermissions,
            None,
        ),
        (
            StatusCode::OK,
            StatusCode::OK,
            IntrospectionBody::MalformedPermissions,
            None,
        ),
        (
            StatusCode::OK,
            StatusCode::OK,
            IntrospectionBody::Malformed,
            None,
        ),
        (
            StatusCode::OK,
            StatusCode::OK,
            IntrospectionBody::Active(true),
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
        let started = SystemTime::now();

        crate::renewal::sweep_once(&auth, &super::empty_session_service()).await;

        let fact = store
            .subject(&SubjectId("subject-a".into()))
            .await
            .unwrap()
            .unwrap();
        assert!(fact.authorization.allows(Capability::ReadAllSessions));
        assert_eq!(fact.tokens.unwrap().refresh_token.unwrap().expose(), "refresh-a");
        assert!(fact.authorization_check_after > started);
        assert!(
            fact.authorization_check_after <= started + Duration::from_secs(61),
            "a transient answer remains due on the next minute sweep"
        );
        server.abort();
    }
}

#[tokio::test]
async fn a_transient_answer_is_selected_again_at_the_next_sweep_deadline() {
    let (unavailable, unavailable_server) = provider(
        StatusCode::FORBIDDEN,
        StatusCode::OK,
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
    let retry_at = due_at + Duration::from_secs(60);
    let budget = AuthOperationBudget::standalone(AUTHORIZATION_CHECK_ATTEMPT_BUDGET);
    let outcome = first_auth
        .reconcile_authorization_with_budget(selected, due_at, retry_at, &budget)
        .await
        .unwrap();
    assert!(matches!(outcome, AuthorizationCheckOutcome::Preserved(_)));
    unavailable_server.abort();

    assert!(store
        .subjects_due_for_authorization_check(
            retry_at - Duration::from_millis(1),
            None,
            1,
        )
        .await
        .unwrap()
        .is_empty());
    let retry = store
        .subjects_due_for_authorization_check(retry_at, None, 1)
        .await
        .unwrap()
        .pop()
        .expect("transient subject is due on the next sweep")
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
        .reconcile_authorization_with_budget(
            retry,
            retry_at,
            retry_at + Duration::from_secs(60),
            &retry_budget,
        )
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
    crate::renewal::sweep_once(&inactive_auth, &super::empty_session_service()).await;
    let inactive_fact = inactive_store
        .subject(&SubjectId("subject-a".into()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(inactive_fact.authorization.capabilities().count(), 0);
    assert!(
        inactive_fact.tokens.is_some(),
        "inactive access-token introspection does not prove the refresh grant invalid"
    );
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
            access_token: SecretValue::new("access-only".into()),
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
        access_token: SecretValue::new("token:subject-a:new".into()),
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
        .reconcile_authorization_with_budget(
            selected,
            SystemTime::now(),
            SystemTime::now() + Duration::from_secs(60),
            &budget,
        )
        .await
        .unwrap();

    assert!(matches!(outcome, AuthorizationCheckOutcome::Skipped));
    let preserved = store
        .subject(&SubjectId("subject-a".into()))
        .await
        .unwrap()
        .unwrap();
    let tokens = preserved.tokens.unwrap();
    assert_eq!(tokens.access_token.expose(), "token:subject-a:new");
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
        .reconcile_authorization_with_budget(
            selected,
            due_at,
            due_at + Duration::from_secs(60),
            &budget,
        )
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
async fn a_rotated_token_generation_is_resolved_again_before_the_cas_retry() {
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
        access_token: SecretValue::new("token:subject-a:new".into()),
        refresh_token: Some(SecretValue::new("refresh-new".into())),
        expires_at: SystemTime::now() + Duration::from_secs(7200),
    });
    store.install_subject_winner_on_replace(1, winner).await;
    let budget = AuthOperationBudget::standalone(AUTHORIZATION_CHECK_ATTEMPT_BUDGET);

    auth.reconcile_authorization_with_budget(
        selected,
        SystemTime::now(),
        SystemTime::now() + Duration::from_secs(60),
        &budget,
    )
    .await
    .unwrap();

    assert_eq!(
        *provider.seen_userinfo_tokens.lock().unwrap(),
        vec![
            "token:subject-a:old".to_string(),
            "token:subject-a:new".to_string(),
        ]
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
async fn a_recreated_subject_incarnation_is_resolved_again_even_at_the_same_generation() {
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
        access_token: SecretValue::new("token:subject-a:new".into()),
        refresh_token: Some(SecretValue::new("refresh-new".into())),
        expires_at: SystemTime::now() + Duration::from_secs(7200),
    });
    store.install_subject_winner_on_replace(1, recreated).await;
    let budget = AuthOperationBudget::standalone(AUTHORIZATION_CHECK_ATTEMPT_BUDGET);

    auth.reconcile_authorization_with_budget(
        selected,
        SystemTime::now(),
        SystemTime::now() + Duration::from_secs(60),
        &budget,
    )
    .await
    .unwrap();

    assert_eq!(
        *provider.seen_userinfo_tokens.lock().unwrap(),
        vec![
            "token:subject-a:old".to_string(),
            "token:subject-a:new".to_string(),
        ]
    );
    let durable = store
        .subject(&SubjectId("subject-a".into()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(durable.incarnation, "recreated-incarnation");
    assert_eq!(durable.revision, 1);
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
        .reconcile_authorization_with_budget(
            selected,
            SystemTime::now(),
            SystemTime::now() + Duration::from_secs(60),
            &budget,
        )
        .await
        .is_err());

    assert_eq!(store.subject_replace_calls(), 3);
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
async fn one_sweep_advances_past_an_unreadable_page_tail_and_drains_the_next_page() {
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
    store
        .fail_subject_read(Some(SubjectId("subject-099".into())))
        .await;

    crate::renewal::sweep_once(&auth, &super::empty_session_service()).await;

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
    assert_eq!(provider.seen_userinfo_tokens.lock().unwrap().len(), 101);
    server.abort();
}
