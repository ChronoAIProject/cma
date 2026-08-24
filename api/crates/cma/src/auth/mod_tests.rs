//! Tests for establishing who is calling.
//!
//! In a sibling file so `mod.rs` stays within the project's file-size limit
//! while what it guarantees stays covered.

use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

use axum::{
    extract::{Path, State},
    http::{header::AUTHORIZATION, StatusCode},
    routing::{delete, get},
    Json, Router,
};
use tokio::sync::{Mutex as AsyncMutex, Semaphore};

use super::*;

fn headers_with_cookie(value: &str) -> HeaderMap {
    let mut h = HeaderMap::new();
    h.insert(
        axum::http::header::COOKIE,
        value.parse().expect("valid header"),
    );
    h
}

#[test]
fn canonical_and_legacy_cookies_are_read_in_migration_order() {
    let h = headers_with_cookie(
        "theme=dark; chrono_bot_session=legacy; cma_session=canonical; other=1",
    );
    assert_eq!(session_cookies(&h), ["canonical", "legacy"]);
}

#[test]
fn a_missing_session_cookie_is_empty() {
    let h = headers_with_cookie("theme=dark; other=1");
    assert!(session_cookies(&h).is_empty());
    assert!(session_cookies(&HeaderMap::new()).is_empty());
}

#[tokio::test]
async fn trusted_mode_attributes_every_request_to_one_subject() {
    let auth = AuthState::new(
        AuthMode::Trusted {
            subject: "local-developer".into(),
        },
        None,
    );
    assert!(!auth.requires_credential());
    let got = auth.resolve(&HeaderMap::new()).await;
    assert_eq!(got, Some(SubjectId("local-developer".into())));
}

#[tokio::test]
async fn provider_mode_rejects_a_request_with_no_cookie() {
    let auth = AuthState::new(
        AuthMode::Provider {
            base_url: "http://provider.invalid".into(),
            client_id: "client".into(),
            client_secret: "secret".into(),
            public_url: "http://service.invalid".into(),
        },
        None,
    );
    assert!(auth.requires_credential());
    assert_eq!(auth.resolve(&HeaderMap::new()).await, None);
}

#[tokio::test]
async fn an_issued_session_resolves_and_stops_resolving_after_logout() {
    let auth = AuthState::new(
        AuthMode::Provider {
            base_url: "http://provider.invalid".into(),
            client_id: "client".into(),
            client_secret: "secret".into(),
            public_url: "http://service.invalid".into(),
        },
        None,
    );
    let token = auth.issue_session(SubjectId("subject-a".into())).await;
    let h = headers_with_cookie(&format!("{SESSION_COOKIE}={token}"));
    assert_eq!(auth.resolve(&h).await, Some(SubjectId("subject-a".into())));

    auth.sessions.write().await.remove(&token);
    assert_eq!(auth.resolve(&h).await, None);
}

#[tokio::test]
async fn a_legacy_cookie_still_resolves_during_the_compatibility_window() {
    let auth = AuthState::new(
        AuthMode::Provider {
            base_url: "http://provider.invalid".into(),
            client_id: "client".into(),
            client_secret: "secret".into(),
            public_url: "http://service.invalid".into(),
        },
        None,
    );
    let token = auth.issue_session(SubjectId("legacy-subject".into())).await;
    let headers = headers_with_cookie(&format!("{LEGACY_SESSION_COOKIE}={token}"));

    assert_eq!(
        auth.resolve(&headers).await,
        Some(SubjectId("legacy-subject".into()))
    );
}

#[tokio::test]
async fn an_invalid_canonical_cookie_does_not_hide_a_valid_legacy_cookie() {
    let auth = AuthState::new(
        AuthMode::Provider {
            base_url: "http://provider.invalid".into(),
            client_id: "client".into(),
            client_secret: "secret".into(),
            public_url: "http://service.invalid".into(),
        },
        None,
    );
    let legacy = auth.issue_session(SubjectId("legacy-subject".into())).await;
    let headers = headers_with_cookie(&format!(
        "{LEGACY_SESSION_COOKIE}={legacy}; {SESSION_COOKIE}=not-issued"
    ));

    assert_eq!(
        auth.resolve(&headers).await,
        Some(SubjectId("legacy-subject".into()))
    );
}

#[tokio::test]
async fn discovery_against_an_unreachable_provider_reports_why() {
    let auth = AuthState::new(
        AuthMode::Provider {
            base_url: "http://127.0.0.1:1".into(),
            client_id: "c".into(),
            client_secret: "s".into(),
            public_url: "http://service.invalid".into(),
        },
        None,
    );
    let err = auth.metadata().await.expect_err("must fail");
    assert!(err.contains("discovery request"), "got {err}");
}

#[test]
fn rebase_moves_a_back_channel_endpoint_onto_the_reachable_host() {
    assert_eq!(
        rebase(
            "http://localhost:8444/oauth/token",
            "http://nyxid.internal:8080"
        ),
        "http://nyxid.internal:8080/oauth/token"
    );
}

#[test]
fn rebase_preserves_the_query_string() {
    assert_eq!(
        rebase("https://public/oauth/userinfo?x=1", "http://internal:3001"),
        "http://internal:3001/oauth/userinfo?x=1"
    );
}

#[test]
fn rebase_tolerates_a_base_with_a_trailing_slash() {
    assert_eq!(
        rebase("https://public/a", "http://internal:1/"),
        "http://internal:1/a"
    );
}

#[test]
fn rebase_handles_an_endpoint_with_no_path() {
    assert_eq!(
        rebase("https://public", "http://internal:1"),
        "http://internal:1"
    );
}

#[test]
fn rebase_leaves_an_unparseable_endpoint_alone() {
    // Failing loudly at request time beats silently rewriting nonsense.
    assert_eq!(rebase("not-a-url", "http://internal:1"), "not-a-url");
}

#[derive(Clone)]
struct RevocationPlatform {
    status: StatusCode,
    attempted: Arc<AsyncMutex<Vec<String>>>,
}

async fn record_revoke(
    State(platform): State<RevocationPlatform>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> StatusCode {
    assert_eq!(
        headers
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok()),
        Some("Bearer retry-owner-token")
    );
    platform.attempted.lock().await.push(id);
    platform.status
}

async fn revocation_platform(
    status: StatusCode,
) -> (
    String,
    Arc<AsyncMutex<Vec<String>>>,
    tokio::task::JoinHandle<()>,
) {
    let attempted = Arc::new(AsyncMutex::new(Vec::new()));
    let app = Router::new()
        .route("/api/v1/api-keys/{id}", delete(record_revoke))
        .with_state(RevocationPlatform {
            status,
            attempted: Arc::clone(&attempted),
        });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind mock identity platform");
    let base = format!("http://{}", listener.local_addr().expect("mock address"));
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serve mock identity platform");
    });
    (base, attempted, server)
}

async fn list_services(headers: HeaderMap) -> Json<serde_json::Value> {
    assert_eq!(
        headers
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok()),
        Some("Bearer retry-owner-token")
    );
    Json(serde_json::json!([{ "id": "service-id", "slug": "service" }]))
}

#[derive(Clone)]
struct MintingPlatform {
    keys: Arc<AsyncMutex<Vec<String>>>,
    delete_attempts: Arc<AsyncMutex<Vec<String>>>,
    session: SessionId,
    started: Arc<Semaphore>,
    finish: Arc<Semaphore>,
}

async fn mint_key(
    State(platform): State<MintingPlatform>,
    headers: HeaderMap,
) -> Json<serde_json::Value> {
    assert_eq!(
        headers
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok()),
        Some("Bearer retry-owner-token")
    );
    platform.keys.lock().await.push("replacement-key".into());
    platform.started.add_permits(1);
    platform
        .finish
        .acquire()
        .await
        .expect("the test still owns the mint semaphore")
        .forget();
    Json(serde_json::json!({
        "id": "replacement-key",
        "full_key": "replacement-value"
    }))
}

async fn list_minted_keys(
    State(platform): State<MintingPlatform>,
    headers: HeaderMap,
) -> Json<serde_json::Value> {
    assert_eq!(
        headers
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok()),
        Some("Bearer retry-owner-token")
    );
    let name = keys::key_name("production", &platform.session);
    Json(serde_json::json!({
        "keys": platform
            .keys
            .lock()
            .await
            .iter()
            .map(|id| serde_json::json!({ "id": id, "name": name }))
            .collect::<Vec<_>>()
    }))
}

async fn delete_minted_key(
    State(platform): State<MintingPlatform>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> StatusCode {
    assert_eq!(
        headers
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok()),
        Some("Bearer retry-owner-token")
    );
    platform.delete_attempts.lock().await.push(id.clone());
    let mut keys = platform.keys.lock().await;
    let index = keys
        .iter()
        .position(|key| key == &id)
        .expect("the provider still owns the key being revoked");
    keys.remove(index);
    StatusCode::NO_CONTENT
}

async fn minting_platform(
    session: &SessionId,
) -> (String, MintingPlatform, tokio::task::JoinHandle<()>) {
    let platform = MintingPlatform {
        keys: Arc::new(AsyncMutex::new(vec!["previous-key".into()])),
        delete_attempts: Arc::new(AsyncMutex::new(Vec::new())),
        session: session.clone(),
        started: Arc::new(Semaphore::new(0)),
        finish: Arc::new(Semaphore::new(0)),
    };
    let app = Router::new()
        .route("/api/v1/user-services", get(list_services))
        .route("/api/v1/api-keys", get(list_minted_keys).post(mint_key))
        .route("/api/v1/api-keys/{id}", delete(delete_minted_key))
        .with_state(platform.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind mock identity platform");
    let base = format!("http://{}", listener.local_addr().expect("mock address"));
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serve mock identity platform");
    });
    (base, platform, server)
}

#[derive(Clone)]
struct BlockingRevocationPlatform {
    attempts: Arc<AtomicUsize>,
    started: Arc<Semaphore>,
    finish: Arc<Semaphore>,
}

async fn block_revoke(
    State(platform): State<BlockingRevocationPlatform>,
    headers: HeaderMap,
) -> StatusCode {
    assert_eq!(
        headers
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok()),
        Some("Bearer retry-owner-token")
    );
    platform.attempts.fetch_add(1, Ordering::SeqCst);
    platform.started.add_permits(1);
    platform
        .finish
        .acquire()
        .await
        .expect("test still owns the release semaphore")
        .forget();
    StatusCode::NO_CONTENT
}

async fn blocking_revocation_platform() -> (
    String,
    BlockingRevocationPlatform,
    tokio::task::JoinHandle<()>,
) {
    let platform = BlockingRevocationPlatform {
        attempts: Arc::new(AtomicUsize::new(0)),
        started: Arc::new(Semaphore::new(0)),
        finish: Arc::new(Semaphore::new(0)),
    };
    let app = Router::new()
        .route("/api/v1/api-keys/{id}", delete(block_revoke))
        .with_state(platform.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind blocking mock identity platform");
    let base = format!("http://{}", listener.local_addr().expect("mock address"));
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serve blocking mock identity platform");
    });
    (base, platform, server)
}

fn retry_tokens() -> SubjectTokens {
    SubjectTokens {
        access_token: "retry-owner-token".into(),
        refresh_token: None,
        expires_at: std::time::Instant::now() + std::time::Duration::from_secs(600),
    }
}

async fn retry_auth(base: &str) -> Arc<AuthState> {
    let auth = AuthState::new(
        AuthMode::Trusted {
            subject: "local".into(),
        },
        None,
    );
    auth.use_issuer(Arc::new(keys::KeyIssuer::new(
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(2))
            .build()
            .expect("test client builds"),
        base,
        "production",
    )))
    .await;
    auth
}

async fn queue_key(auth: &AuthState, id: &str, owner: &SubjectId) {
    auth.superseded
        .write()
        .await
        .push((id.into(), owner.clone()));
}

async fn closed_local_url() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("reserve a local address");
    let url = format!(
        "http://{}",
        listener.local_addr().expect("reserved address")
    );
    drop(listener);
    url
}

#[tokio::test]
async fn a_confirmed_superseded_revoke_is_counted_once_and_retired() {
    let (base, attempted, server) = revocation_platform(StatusCode::NO_CONTENT).await;
    let auth = retry_auth(&base).await;
    let owner = SubjectId("retry-owner".into());
    auth.store_tokens(&owner, retry_tokens()).await;
    queue_key(&auth, "superseded-key", &owner).await;

    assert_eq!(auth.take_back_superseded().await, 1);
    assert!(auth.superseded.read().await.is_empty());
    assert_eq!(auth.take_back_superseded().await, 0);
    assert_eq!(attempted.lock().await.as_slice(), ["superseded-key"]);
    server.abort();
}

#[tokio::test]
async fn a_not_found_superseded_key_remains_retry_debt() {
    let (base, attempted, server) = revocation_platform(StatusCode::NOT_FOUND).await;
    let auth = retry_auth(&base).await;
    let owner = SubjectId("retry-owner".into());
    auth.store_tokens(&owner, retry_tokens()).await;
    queue_key(&auth, "superseded-key", &owner).await;

    assert_eq!(auth.take_back_superseded().await, 0);
    assert_eq!(auth.superseded.read().await.len(), 1);
    assert_eq!(attempted.lock().await.as_slice(), ["superseded-key"]);
    server.abort();
}

#[tokio::test]
async fn cancelling_reissue_after_provider_mint_leaves_resumable_progress() {
    let session = SessionId("sesn_reissue_transfer".into());
    let (base, platform, server) = minting_platform(&session).await;
    let auth = retry_auth(&base).await;
    let owner = SubjectId("retry-owner".into());
    auth.store_tokens(&owner, retry_tokens()).await;
    auth.issued.write().await.insert(
        session.clone(),
        Issued {
            id: "previous-key".into(),
            value: "previous-value".into(),
            owner: owner.clone(),
            minted_at: std::time::Instant::now(),
        },
    );

    let reissue = tokio::spawn({
        let auth = Arc::clone(&auth);
        let session = session.clone();
        let owner = owner.clone();
        async move {
            auth.reissue(&session, &owner, &[layer2_core::ServiceId::new("service")])
                .await
        }
    });
    platform
        .started
        .acquire()
        .await
        .expect("the provider records the replacement mint")
        .forget();
    assert_eq!(
        platform.keys.lock().await.as_slice(),
        ["previous-key", "replacement-key"],
        "the provider has both no-expiry keys before local progress is recorded"
    );

    let progress_gate = auth.reissues.write().await;
    assert!(progress_gate.minting.contains(&session));
    platform.finish.add_permits(1);
    tokio::task::yield_now().await;
    reissue.abort();
    assert!(reissue
        .await
        .expect_err("the reissue was cancelled")
        .is_cancelled());
    drop(progress_gate);

    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        loop {
            let progress = auth.reissues.read().await;
            if progress.pending.contains_key(&session) && !progress.minting.contains(&session) {
                break;
            }
            drop(progress);
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the detached mint task records bounded progress and completes");

    let issued = auth.issued.read().await;
    let held = issued.get(&session).expect("the previous key stays owned");
    assert_eq!(held.id, "previous-key");
    assert_eq!(held.value, "previous-value");
    drop(issued);
    assert!(auth.superseded.read().await.is_empty());

    let progress = auth.reissues.read().await;
    let pending = progress
        .pending
        .get(&session)
        .expect("the replacement remains available for delivery");
    assert_eq!(pending.id, "replacement-key");
    assert_eq!(pending.value, "replacement-value");
    assert!(progress.minting.is_empty());
    assert!(progress.retiring.is_empty());
    drop(progress);

    assert_eq!(
        auth.reissue(&session, &owner, &[layer2_core::ServiceId::new("service")])
            .await,
        Some("replacement-value".into()),
        "the next sweep resumes the recorded replacement without another mint"
    );
    assert!(auth.confirm_reissue(&session).await);
    assert_eq!(
        auth.issued
            .read()
            .await
            .get(&session)
            .map(|held| held.id.as_str()),
        Some("replacement-key")
    );
    assert_eq!(
        auth.superseded.read().await.as_slice(),
        [("previous-key".into(), owner)]
    );
    let progress = auth.reissues.read().await;
    assert!(progress.pending.is_empty());
    assert!(progress.minting.is_empty());
    assert!(progress.retiring.is_empty());
    assert_eq!(
        platform.keys.lock().await.as_slice(),
        ["previous-key", "replacement-key"],
        "local promotion neither remints nor loses either provider key"
    );
    server.abort();
}

#[tokio::test]
async fn ending_one_session_does_not_revoke_another_sessions_rotation_debt() {
    let (base, attempted, server) = revocation_platform(StatusCode::NO_CONTENT).await;
    let auth = retry_auth(&base).await;
    let owner = SubjectId("retry-owner".into());
    let rotating = SessionId("sesn_rotating".into());
    let ending = SessionId("sesn_ending".into());
    auth.store_tokens(&owner, retry_tokens()).await;

    auth.issued.write().await.insert(
        rotating.clone(),
        Issued {
            id: "rotating-old-key".into(),
            value: "rotating-old-value".into(),
            owner: owner.clone(),
            minted_at: std::time::Instant::now(),
        },
    );
    auth.reissues.write().await.pending.insert(
        rotating.clone(),
        Issued {
            id: "rotating-replacement-key".into(),
            value: "rotating-replacement-value".into(),
            owner: owner.clone(),
            minted_at: std::time::Instant::now(),
        },
    );
    assert!(auth.confirm_reissue(&rotating).await);
    auth.issued.write().await.insert(
        ending.clone(),
        Issued {
            id: "ending-key".into(),
            value: "ending-value".into(),
            owner: owner.clone(),
            minted_at: std::time::Instant::now(),
        },
    );

    layer2_core::CredentialSource::release(&*auth, &ending).await;

    assert!(attempted.lock().await.is_empty());
    assert_eq!(
        auth.issued
            .read()
            .await
            .get(&rotating)
            .map(|held| held.id.as_str()),
        Some("rotating-replacement-key")
    );
    assert!(!auth.issued.read().await.contains_key(&ending));
    assert_eq!(
        auth.superseded.read().await.as_slice(),
        [
            ("rotating-old-key".into(), owner.clone()),
            ("ending-key".into(), owner),
        ],
        "release only transfers its own key and leaves every DELETE to the periodic sweep"
    );

    assert_eq!(auth.take_back_superseded().await, 2);
    assert_eq!(
        attempted.lock().await.as_slice(),
        ["rotating-old-key", "ending-key"]
    );
    server.abort();
}

#[tokio::test]
async fn release_during_an_in_flight_reissue_retires_both_provider_keys() {
    let session = SessionId("sesn_reissue_release".into());
    let (base, platform, server) = minting_platform(&session).await;
    let auth = retry_auth(&base).await;
    let owner = SubjectId("retry-owner".into());
    auth.store_tokens(&owner, retry_tokens()).await;
    auth.issued.write().await.insert(
        session.clone(),
        Issued {
            id: "previous-key".into(),
            value: "previous-value".into(),
            owner: owner.clone(),
            minted_at: std::time::Instant::now(),
        },
    );

    let reissue = tokio::spawn({
        let auth = Arc::clone(&auth);
        let session = session.clone();
        let owner = owner.clone();
        async move {
            auth.reissue(&session, &owner, &[layer2_core::ServiceId::new("service")])
                .await
        }
    });
    platform
        .started
        .acquire()
        .await
        .expect("the replacement exists at the provider")
        .forget();
    auth.tokens.write().await.remove(&owner);

    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        layer2_core::CredentialSource::release(&*auth, &session).await;
    })
    .await
    .expect("release records the in-flight mint without waiting for the provider");

    assert!(!auth.issued.read().await.contains_key(&session));
    assert_eq!(
        auth.superseded.read().await.as_slice(),
        [("previous-key".into(), owner.clone())]
    );
    let progress = auth.reissues.read().await;
    assert!(progress.pending.is_empty());
    assert_eq!(progress.minting.iter().collect::<Vec<_>>(), [&session]);
    assert_eq!(progress.retiring.iter().collect::<Vec<_>>(), [&session]);
    drop(progress);
    assert_eq!(
        platform.keys.lock().await.as_slice(),
        ["previous-key", "replacement-key"]
    );
    assert!(platform.delete_attempts.lock().await.is_empty());

    auth.store_tokens(&owner, retry_tokens()).await;
    assert_eq!(
        auth.reclaim_orphaned_credentials(&reclaim_tests::empty_session_service())
            .await,
        0,
        "the replacement remains owned while its mint completion is retiring"
    );
    assert!(platform.delete_attempts.lock().await.is_empty());

    platform.finish.add_permits(1);
    assert_eq!(
        reissue.await.expect("the detached mint task completes"),
        None,
        "a replacement for a released session must not be delivered"
    );

    assert_eq!(
        auth.superseded.read().await.as_slice(),
        [
            ("previous-key".into(), owner.clone()),
            ("replacement-key".into(), owner.clone()),
        ]
    );
    let progress = auth.reissues.read().await;
    assert!(progress.pending.is_empty());
    assert!(progress.minting.is_empty());
    assert!(progress.retiring.is_empty());
    drop(progress);
    assert_eq!(
        platform.keys.lock().await.as_slice(),
        ["previous-key", "replacement-key"]
    );

    assert_eq!(auth.take_back_superseded().await, 2);
    assert!(auth.superseded.read().await.is_empty());
    assert!(platform.keys.lock().await.is_empty());
    assert_eq!(
        platform.delete_attempts.lock().await.as_slice(),
        ["previous-key", "replacement-key"]
    );
    server.abort();
}

#[tokio::test]
async fn a_failed_superseded_revoke_stays_owed_until_a_later_pass_succeeds() {
    let unreachable = closed_local_url().await;
    let auth = retry_auth(&unreachable).await;
    let owner = SubjectId("retry-owner".into());
    auth.store_tokens(&owner, retry_tokens()).await;
    queue_key(&auth, "superseded-key", &owner).await;

    assert_eq!(auth.take_back_superseded().await, 0);
    assert_eq!(
        auth.superseded.read().await.as_slice(),
        [("superseded-key".into(), owner.clone())]
    );

    let (working, attempted, server) = revocation_platform(StatusCode::NO_CONTENT).await;
    auth.use_issuer(Arc::new(keys::KeyIssuer::new(
        reqwest::Client::new(),
        working,
        "production",
    )))
    .await;

    assert_eq!(auth.take_back_superseded().await, 1);
    assert!(auth.superseded.read().await.is_empty());
    assert_eq!(attempted.lock().await.as_slice(), ["superseded-key"]);
    server.abort();
}

#[tokio::test]
async fn a_tokenless_superseded_key_waits_for_its_owners_token() {
    let (base, attempted, server) = revocation_platform(StatusCode::NO_CONTENT).await;
    let auth = retry_auth(&base).await;
    let owner = SubjectId("retry-owner".into());
    queue_key(&auth, "superseded-key", &owner).await;

    assert_eq!(auth.take_back_superseded().await, 0);
    assert_eq!(auth.superseded.read().await.len(), 1);
    assert!(attempted.lock().await.is_empty());

    auth.store_tokens(&owner, retry_tokens()).await;
    assert_eq!(auth.take_back_superseded().await, 1);
    assert!(auth.superseded.read().await.is_empty());
    assert_eq!(attempted.lock().await.as_slice(), ["superseded-key"]);
    server.abort();
}

#[tokio::test]
async fn cancelling_a_sweep_does_not_detach_its_unconfirmed_debt() {
    let (base, platform, server) = blocking_revocation_platform().await;
    let auth = retry_auth(&base).await;
    let owner = SubjectId("retry-owner".into());
    auth.store_tokens(&owner, retry_tokens()).await;
    queue_key(&auth, "superseded-key", &owner).await;

    let sweep = tokio::spawn({
        let auth = Arc::clone(&auth);
        async move { auth.take_back_superseded().await }
    });
    platform
        .started
        .acquire()
        .await
        .expect("the request reaches the platform")
        .forget();
    sweep.abort();
    assert!(
        sweep
            .await
            .expect_err("the sweep was cancelled")
            .is_cancelled(),
        "the task must stop while its DELETE is unconfirmed"
    );
    assert_eq!(auth.superseded.read().await.len(), 1);
    server.abort();

    let (working, attempted, working_server) = revocation_platform(StatusCode::NO_CONTENT).await;
    auth.use_issuer(Arc::new(keys::KeyIssuer::new(
        reqwest::Client::new(),
        working,
        "production",
    )))
    .await;
    assert_eq!(auth.take_back_superseded().await, 1);
    assert_eq!(attempted.lock().await.as_slice(), ["superseded-key"]);
    working_server.abort();
}

#[tokio::test]
async fn overlapping_sweeps_attempt_and_count_one_key_only_once() {
    let (base, platform, server) = blocking_revocation_platform().await;
    let auth = retry_auth(&base).await;
    let owner = SubjectId("retry-owner".into());
    auth.store_tokens(&owner, retry_tokens()).await;
    queue_key(&auth, "superseded-key", &owner).await;

    let first = tokio::spawn({
        let auth = Arc::clone(&auth);
        async move { auth.take_back_superseded().await }
    });
    platform
        .started
        .acquire()
        .await
        .expect("the first request reaches the platform")
        .forget();
    let second = tokio::spawn({
        let auth = Arc::clone(&auth);
        async move { auth.take_back_superseded().await }
    });
    tokio::task::yield_now().await;
    assert_eq!(platform.attempts.load(Ordering::SeqCst), 1);

    platform.finish.add_permits(1);
    assert_eq!(first.await.expect("first sweep completes"), 1);
    assert_eq!(second.await.expect("second sweep completes"), 0);
    assert_eq!(platform.attempts.load(Ordering::SeqCst), 1);
    assert!(auth.superseded.read().await.is_empty());
    server.abort();
}

#[path = "reclaim_tests.rs"]
mod reclaim_tests;
