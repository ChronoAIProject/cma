use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
};

use axum::{
    http::{header, HeaderMap, HeaderValue, StatusCode},
    routing::{get, post},
    Json, Router,
};
use layer2_core::{Authorization, Capability, SubjectId};
use serde_json::{json, Value};
use tokio::sync::Semaphore;

use super::*;
use crate::auth::persistence::{AuthStore, MemoryAuthStore, SubjectRefreshLease};
use crate::auth::{profile::Profile, ProviderMetadata};

struct Fixture {
    store: Arc<MemoryAuthStore>,
    first: Arc<AuthState>,
    second: Arc<AuthState>,
    calls: Arc<AtomicUsize>,
    requirements_reads: Arc<Mutex<Vec<Option<HeaderValue>>>>,
    started: Vec<Arc<Semaphore>>,
    release: Vec<Arc<Semaphore>>,
    server: tokio::task::JoinHandle<()>,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}

fn tokens(access: &str, refresh: Option<&str>, expired: bool) -> DurableSubjectTokens {
    DurableSubjectTokens {
        access_token: SecretValue::new(access.into()),
        refresh_token: refresh.map(|value| SecretValue::new(value.into())),
        expires_at: if expired {
            SystemTime::UNIX_EPOCH
        } else {
            SystemTime::now() + Duration::from_secs(3600)
        },
    }
}

fn initial() -> SubjectAuthState {
    SubjectAuthState::new(
        SubjectId::new("refresh-owner"),
        Authorization::new([Capability::CreateSandbox]),
        Profile {
            display_name: Some("Before".into()),
            ..Default::default()
        },
        Some(tokens(
            "fixture-old-access",
            Some("fixture-old-refresh"),
            true,
        )),
    )
}

fn success(refresh: Option<&str>) -> (StatusCode, Value) {
    let mut body = json!({ "access_token": "fixture-renewed-access", "expires_in": 3600 });
    if let Some(refresh) = refresh {
        body["refresh_token"] = json!(refresh);
    }
    (StatusCode::OK, body)
}

impl Fixture {
    async fn new(replies: Vec<(StatusCode, Value)>) -> Self {
        let store = MemoryAuthStore::default();
        store.put_subject(initial()).await.unwrap();
        Self::with_store(replies, store).await
    }

    async fn with_store(replies: Vec<(StatusCode, Value)>, store: MemoryAuthStore) -> Self {
        let calls = Arc::new(AtomicUsize::new(0));
        let requirements_reads = Arc::new(Mutex::new(Vec::new()));
        let started: Vec<_> = replies
            .iter()
            .map(|_| Arc::new(Semaphore::new(0)))
            .collect();
        let release: Vec<_> = replies
            .iter()
            .map(|_| Arc::new(Semaphore::new(0)))
            .collect();
        let app = Router::new()
            .route(
                "/token",
                post({
                    let calls = calls.clone();
                    let started = started.clone();
                    let release = release.clone();
                    move || {
                        let index = calls.fetch_add(1, Ordering::SeqCst);
                        let reply = replies[index].clone();
                        let started = started[index].clone();
                        let release = release[index].clone();
                        async move {
                            started.add_permits(1);
                            release.acquire().await.unwrap().forget();
                            (reply.0, Json(reply.1))
                        }
                    }
                }),
            )
            .route(
                "/api/v1/user-services",
                get({
                    let reads = requirements_reads.clone();
                    move |headers: HeaderMap| {
                        reads
                            .lock()
                            .unwrap()
                            .push(headers.get(header::AUTHORIZATION).cloned());
                        async { Json(json!({"services": []})) }
                    }
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let store = Arc::new(store);
        let mode = AuthMode::Provider {
            base_url: base.clone(),
            client_id: "fixture-client".into(),
            client_secret: "fixture-secret".into(),
            public_url: "https://cma.example.test".into(),
        };
        let first = AuthState::with_store(mode.clone(), None, store.clone());
        let second = AuthState::with_store(mode, None, store.clone());
        for auth in [&first, &second] {
            *auth.fixture_metadata().write().await = Some(ProviderMetadata {
                authorization_endpoint: format!("{base}/authorize"),
                token_endpoint: format!("{base}/token"),
                userinfo_endpoint: None,
                introspection_endpoint: None,
            });
        }
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self {
            store,
            first,
            second,
            calls,
            requirements_reads,
            started,
            release,
            server,
        }
    }

    async fn state(&self) -> SubjectAuthState {
        self.store
            .subject(&SubjectId::new("refresh-owner"))
            .await
            .unwrap()
            .unwrap()
    }

    async fn started(&self, index: usize) {
        tokio::time::timeout(Duration::from_secs(2), self.started[index].acquire())
            .await
            .unwrap()
            .unwrap()
            .forget();
    }

    async fn expire_lease(&self) {
        let current = self.state().await;
        let mut next = current.clone();
        next.revision += 1;
        next.refresh_lease.as_mut().unwrap().expires_at =
            SystemTime::UNIX_EPOCH + Duration::from_secs(1);
        assert!(self
            .store
            .replace_subject(&current.incarnation, current.revision, next)
            .await
            .unwrap());
    }
}

#[tokio::test]
async fn requirements_deadline_allows_admitted_refresh_to_settle_before_refusing_reads() {
    use crate::trigger::CmaServiceConnectionBroker;
    use layer2_core::trigger_domain::{
        ServiceConnectionBroker, ServiceConnectionError, ServiceRequirementState,
    };
    use layer2_core::ServiceId;

    let observed = initial();
    let entered = Arc::new(Semaphore::new(0));
    let release = Arc::new(Semaphore::new(0));
    let store = MemoryAuthStore::default().with_refresh_completion_gate(
        observed.token_generation + 1,
        entered.clone(),
        release.clone(),
    );
    store.put_subject(observed.clone()).await.unwrap();
    let fixture = Fixture::with_store(vec![success(Some("fixture-rotated"))], store).await;
    let broker = Arc::new(
        CmaServiceConnectionBroker::from_auth_config(
            fixture.first.fixture_mode(),
            Some("https://nyxid.example.test"),
            fixture.first.clone(),
        )
        .unwrap(),
    );
    let services = [ServiceId::new("fixture-service")];
    let mut call = tokio::spawn({
        let broker = broker.clone();
        let owner = observed.subject.clone();
        let services = services.clone();
        async move { broker.requirements(&owner, &services).await }
    });
    fixture.started(0).await;
    fixture.release[0].add_permits(1);
    tokio::time::timeout(Duration::from_secs(2), entered.acquire())
        .await
        .unwrap()
        .unwrap()
        .forget();
    let pending = fixture.state().await;
    assert_eq!(pending.token_generation, observed.token_generation);
    assert!(pending.refresh_lease.is_some());

    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(16)).await;
    tokio::task::yield_now().await;
    assert!(futures_util::poll!(&mut call).is_pending());
    assert!(fixture.requirements_reads.lock().unwrap().is_empty());
    release.add_permits(1);
    let result = tokio::time::timeout(Duration::from_secs(2), call)
        .await
        .unwrap()
        .unwrap();
    tokio::time::resume();
    assert!(matches!(
        result,
        Err(ServiceConnectionError::Unavailable(_))
    ));
    assert!(fixture.requirements_reads.lock().unwrap().is_empty());

    let settled = fixture.state().await;
    assert_eq!(settled.token_generation, observed.token_generation + 1);
    assert!(settled.refresh_lease.is_none());
    let tokens = settled.tokens.unwrap();
    assert_eq!(tokens.access_token.expose(), "fixture-renewed-access");
    assert_eq!(tokens.refresh_token.unwrap().expose(), "fixture-rotated");
    let requirements = broker
        .requirements(&observed.subject, &services)
        .await
        .unwrap();
    assert_eq!(requirements[0].state, ServiceRequirementState::Missing);
    assert_eq!(fixture.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        fixture.requirements_reads.lock().unwrap().as_slice(),
        &[Some(HeaderValue::from_static(
            "Bearer fixture-renewed-access"
        ))]
    );
}

fn run(
    auth: Arc<AuthState>,
    mut state: SubjectAuthState,
) -> tokio::task::JoinHandle<Result<SubjectAuthState, StoreError>> {
    tokio::spawn(async move {
        refresh_if_needed(&auth, &mut state).await?;
        Ok(state)
    })
}

async fn joined(
    task: tokio::task::JoinHandle<Result<SubjectAuthState, StoreError>>,
) -> Result<SubjectAuthState, StoreError> {
    tokio::time::timeout(Duration::from_secs(3), task)
        .await
        .unwrap()
        .unwrap()
}

#[tokio::test]
async fn two_replicas_refresh_each_generation_once_with_rotating_or_reused_tokens() {
    for refresh in [Some("fixture-rotated"), Some("fixture-old-refresh"), None] {
        let fixture = Fixture::new(vec![success(refresh), success(refresh)]).await;
        let observed = fixture.state().await;
        let first = run(fixture.first.clone(), observed.clone());
        fixture.started(0).await;
        let second = run(fixture.second.clone(), observed.clone());
        fixture.release[0].add_permits(1);
        fixture.release[1].add_permits(1);
        let first = joined(first).await.unwrap();
        let second = joined(second).await.unwrap();
        assert_eq!(fixture.calls.load(Ordering::SeqCst), 1);
        assert_eq!(first.tokens, second.tokens);
        assert_eq!(first.token_generation, observed.token_generation + 1);
        assert_eq!(second.token_generation, first.token_generation);
        assert!(fixture.state().await.refresh_lease.is_none());
    }
}

#[tokio::test]
async fn stale_same_token_same_generation_incarnation_never_calls_provider() {
    let fixture = Fixture::new(vec![success(None)]).await;
    let mut stale = fixture.state().await;
    let mut recreated = initial();
    recreated.tokens = stale.tokens.clone();
    fixture.store.put_subject(recreated.clone()).await.unwrap();
    refresh_if_needed(&fixture.first, &mut stale).await.unwrap();
    assert_eq!(fixture.calls.load(Ordering::SeqCst), 0);
    assert_eq!(stale.incarnation, recreated.incarnation);
}

#[tokio::test]
async fn delayed_success_or_invalid_grant_cannot_overwrite_a_new_login() {
    for reply in [
        success(Some("fixture-stale-refresh")),
        (StatusCode::BAD_REQUEST, json!({"error": "invalid_grant"})),
    ] {
        let fixture = Fixture::new(vec![reply]).await;
        let original = fixture.state().await;
        let pending = run(fixture.first.clone(), original.clone());
        fixture.started(0).await;
        fixture
            .second
            .store_sign_in(
                original.subject.clone(),
                tokens("fixture-login-access", Some("fixture-login-refresh"), false),
                Authorization::new([Capability::LaunchTriggers]),
                Profile {
                    display_name: Some("New Login".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let winner = fixture.state().await;
        fixture.release[0].add_permits(1);
        let returned = joined(pending).await.unwrap();
        let stored = fixture.state().await;
        assert_ne!(winner.incarnation, original.incarnation);
        for state in [returned, stored] {
            assert_eq!(state.incarnation, winner.incarnation);
            assert_eq!(state.revision, winner.revision);
            assert_eq!(state.token_generation, winner.token_generation);
            assert_eq!(state.tokens, winner.tokens);
            assert_eq!(state.authorization, winner.authorization);
            assert_eq!(state.profile, winner.profile);
            assert!(state.refresh_lease.is_none());
        }
    }
}

#[tokio::test]
async fn takeover_fences_delayed_success_rejection_and_abandonment() {
    for reply in [
        success(Some("fixture-stale-refresh")),
        (StatusCode::BAD_REQUEST, json!({"error": "invalid_grant"})),
        (
            StatusCode::SERVICE_UNAVAILABLE,
            json!({"error": "invalid_grant"}),
        ),
    ] {
        let fixture = Fixture::new(vec![reply, success(Some("fixture-winner-refresh"))]).await;
        let observed = fixture.state().await;
        let first = run(fixture.first.clone(), observed.clone());
        fixture.started(0).await;
        fixture.expire_lease().await;
        let second = run(fixture.second.clone(), observed);
        fixture.started(1).await;
        fixture.release[1].add_permits(1);
        let winner = joined(second).await.unwrap();
        fixture.release[0].add_permits(1);
        let returned = joined(first).await.unwrap();
        assert_eq!(returned.tokens, winner.tokens);
        assert_eq!(returned.token_generation, winner.token_generation);
        assert_eq!(fixture.state().await.revision, winner.revision);
    }
}

#[tokio::test]
async fn stale_reply_cannot_release_a_successors_unfinished_claim() {
    for reply in [
        success(Some("fixture-stale-refresh")),
        (StatusCode::BAD_REQUEST, json!({"error": "invalid_grant"})),
        (
            StatusCode::SERVICE_UNAVAILABLE,
            json!({"error": "invalid_grant"}),
        ),
    ] {
        let fixture = Fixture::new(vec![reply, success(Some("fixture-winner-refresh"))]).await;
        let observed = fixture.state().await;
        let first = run(fixture.first.clone(), observed.clone());
        fixture.started(0).await;
        fixture.expire_lease().await;
        let second = run(fixture.second.clone(), observed);
        fixture.started(1).await;
        let successor = fixture.state().await;
        fixture.release[0].add_permits(1);
        assert!(joined(first).await.is_err());
        let retained = fixture.state().await;
        assert_eq!(retained.refresh_lease, successor.refresh_lease);
        assert_eq!(retained.revision, successor.revision);
        assert_eq!(retained.tokens, successor.tokens);
        fixture.release[1].add_permits(1);
        assert!(joined(second).await.unwrap().refresh_lease.is_none());
    }
}

#[tokio::test]
async fn transport_failure_releases_only_its_claim_and_preserves_authority() {
    let fixture = Fixture::new(vec![success(None)]).await;
    fixture
        .first
        .fixture_metadata()
        .write()
        .await
        .as_mut()
        .unwrap()
        .token_endpoint = "http://127.0.0.1:0/token".into();
    let mut state = fixture.state().await;
    let original = state.clone();
    assert!(matches!(
        refresh_if_needed(&fixture.first, &mut state).await,
        Err(StoreError::Unavailable(_))
    ));
    let retained = fixture.state().await;
    assert_eq!(retained.tokens, original.tokens);
    assert_eq!(retained.authorization, original.authorization);
    assert_eq!(retained.token_generation, original.token_generation);
    assert!(retained.refresh_lease.is_none());
}

#[tokio::test]
async fn expired_claim_cannot_publish_even_before_a_takeover() {
    let fixture = Fixture::new(vec![
        success(Some("fixture-stale-refresh")),
        success(Some("fixture-winner-refresh")),
    ])
    .await;
    let observed = fixture.state().await;
    let first = run(fixture.first.clone(), observed.clone());
    fixture.started(0).await;
    fixture.expire_lease().await;
    fixture.release[0].add_permits(1);
    assert!(matches!(
        joined(first).await,
        Err(StoreError::Unavailable(_))
    ));
    assert_eq!(fixture.state().await.tokens, observed.tokens);
    assert_eq!(
        fixture.state().await.token_generation,
        observed.token_generation
    );
    let second = run(fixture.second.clone(), observed);
    fixture.started(1).await;
    fixture.release[1].add_permits(1);
    assert!(joined(second).await.unwrap().refresh_lease.is_none());
}

#[tokio::test]
async fn cancellation_leaves_bounded_claim_that_another_replica_can_take_over() {
    let fixture = Fixture::new(vec![success(None), success(Some("fixture-winner-refresh"))]).await;
    let observed = fixture.state().await;
    let first = run(fixture.first.clone(), observed.clone());
    fixture.started(0).await;
    first.abort();
    assert!(first.await.unwrap_err().is_cancelled());
    assert!(fixture.state().await.refresh_lease.is_some());
    fixture.expire_lease().await;
    let second = run(fixture.second.clone(), observed);
    fixture.started(1).await;
    fixture.release[1].add_permits(1);
    assert!(joined(second).await.unwrap().refresh_lease.is_none());
}

#[tokio::test]
async fn refresh_preserves_grants_and_profile_updated_during_the_exchange() {
    let fixture = Fixture::new(vec![success(None)]).await;
    let pending = run(fixture.first.clone(), fixture.state().await);
    fixture.started(0).await;
    let current = fixture.state().await;
    let mut next = current.clone();
    next.revision += 1;
    next.authorization = Authorization::none();
    next.profile.display_name = Some("Updated While Refreshing".into());
    assert!(fixture
        .store
        .replace_subject(&current.incarnation, current.revision, next.clone())
        .await
        .unwrap());
    fixture.release[0].add_permits(1);
    let returned = joined(pending).await.unwrap();
    assert_eq!(returned.authorization, next.authorization);
    assert_eq!(returned.profile, next.profile);
}

#[tokio::test]
async fn only_terminal_rejection_discards_tokens_and_authorization() {
    for status in [StatusCode::BAD_REQUEST] {
        let fixture = Fixture::new(vec![(status, json!({"error": "invalid_grant"}))]).await;
        let original = fixture.state().await;
        let pending = run(fixture.first.clone(), original.clone());
        fixture.started(0).await;
        fixture.release[0].add_permits(1);
        let state = joined(pending).await.unwrap();
        assert!(state.tokens.is_none());
        assert_eq!(state.authorization, Authorization::none());
        assert_eq!(state.token_generation, original.token_generation + 1);
        assert!(state.refresh_lease.is_none());
    }
    for reply in [
        (StatusCode::UNAUTHORIZED, json!({"error": "invalid_grant"})),
        (
            StatusCode::TOO_MANY_REQUESTS,
            json!({"error": "invalid_grant"}),
        ),
        (
            StatusCode::SERVICE_UNAVAILABLE,
            json!({"error": "invalid_grant"}),
        ),
        (StatusCode::BAD_REQUEST, json!({"error": "invalid_client"})),
        (StatusCode::OK, json!({"access_token": ""})),
        (
            StatusCode::OK,
            json!({"access_token": "fixture-renewed", "refresh_token": " "}),
        ),
        (
            StatusCode::OK,
            json!({"access_token": "fixture-renewed", "expires_in": 0}),
        ),
        (
            StatusCode::OK,
            json!({"access_token": "fixture-renewed", "expires_in": u64::MAX}),
        ),
        (StatusCode::OK, json!({"access_token": 42})),
    ] {
        let fixture = Fixture::new(vec![reply]).await;
        let original = fixture.state().await;
        let pending = run(fixture.first.clone(), original.clone());
        fixture.started(0).await;
        fixture.release[0].add_permits(1);
        assert!(joined(pending).await.is_err());
        let state = fixture.state().await;
        assert_eq!(state.tokens, original.tokens);
        assert_eq!(state.authorization, original.authorization);
        assert_eq!(state.token_generation, original.token_generation);
        assert!(state.refresh_lease.is_none());
    }
}

#[tokio::test]
async fn nonrenewable_access_is_kept_until_actual_expiry_then_authority_is_cleared() {
    let fixture = Fixture::new(vec![success(None)]).await;
    let mut state = fixture.state().await;
    state.tokens = Some(tokens("fixture-unrenewable", None, false));
    state.tokens.as_mut().unwrap().expires_at = SystemTime::now() + Duration::from_secs(30);
    fixture.store.put_subject(state.clone()).await.unwrap();
    refresh_if_needed(&fixture.first, &mut state).await.unwrap();
    assert!(state.tokens.is_some());
    assert!(state.authorization.allows(Capability::CreateSandbox));
    state.tokens.as_mut().unwrap().expires_at = SystemTime::UNIX_EPOCH;
    fixture.store.put_subject(state.clone()).await.unwrap();
    refresh_if_needed(&fixture.first, &mut state).await.unwrap();
    assert!(state.tokens.is_none());
    assert_eq!(state.authorization, Authorization::none());
    assert_eq!(fixture.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test(start_paused = true)]
async fn live_lease_wait_is_bounded_without_mutating_custody() {
    let fixture = Fixture::new(vec![success(None)]).await;
    let mut state = fixture.state().await;
    state.refresh_lease = Some(SubjectRefreshLease {
        holder: "other-replica".into(),
        expires_at: SystemTime::now() + Duration::from_secs(300),
    });
    fixture.store.put_subject(state.clone()).await.unwrap();
    let original = state.clone();
    assert!(matches!(
        refresh_if_needed(&fixture.first, &mut state).await,
        Err(StoreError::Unavailable(_))
    ));
    let stored = fixture.state().await;
    assert_eq!(stored.tokens, original.tokens);
    assert_eq!(stored.refresh_lease, original.refresh_lease);
    assert_eq!(stored.revision, original.revision);
    assert_eq!(fixture.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn exhausted_revision_fails_closed_before_provider_exchange() {
    let fixture = Fixture::new(vec![success(None)]).await;
    let mut state = fixture.state().await;
    state.revision = i64::MAX as u64;
    fixture.store.put_subject(state.clone()).await.unwrap();
    assert!(matches!(
        refresh_if_needed(&fixture.first, &mut state).await,
        Err(StoreError::Malformed(_))
    ));
    assert_eq!(fixture.calls.load(Ordering::SeqCst), 0);
    assert!(fixture.state().await.refresh_lease.is_none());
}
