//! Tests for establishing who is calling through durable state.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use axum::http::HeaderMap;

use super::persistence::{AuthStore, BrowserSession, MemoryAuthStore, SecretDigest};
use super::*;

fn headers_with_cookie(value: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        axum::http::header::COOKIE,
        value.parse().expect("valid cookie header"),
    );
    headers
}

fn provider(store: Arc<dyn AuthStore>) -> Arc<AuthState> {
    AuthState::with_store(
        AuthMode::Provider {
            base_url: "http://provider.invalid".into(),
            client_id: "client".into(),
            client_secret: "secret".into(),
            public_url: "http://service.invalid".into(),
        },
        None,
        store,
    )
}

#[test]
fn canonical_and_legacy_cookies_are_read_in_migration_order() {
    let headers = headers_with_cookie(
        "theme=dark; chrono_bot_session=legacy; cma_session=canonical; other=1",
    );
    assert_eq!(session_cookies(&headers), ["canonical", "legacy"]);
}

#[test]
fn a_missing_session_cookie_is_empty() {
    let headers = headers_with_cookie("theme=dark; other=1");
    assert!(session_cookies(&headers).is_empty());
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
    assert_eq!(
        auth.resolve(&HeaderMap::new())
            .await
            .expect("trusted mode does not need a lookup"),
        Some(SubjectId("local-developer".into()))
    );
}

#[tokio::test]
async fn provider_mode_rejects_a_request_with_no_credential() {
    let auth = provider(Arc::new(MemoryAuthStore::default()));
    assert_eq!(
        auth.resolve(&HeaderMap::new()).await.expect("store works"),
        None
    );
}

#[tokio::test]
async fn a_cookie_issued_by_one_instance_resolves_on_another() {
    let store = Arc::new(MemoryAuthStore::default());
    let first = provider(store.clone());
    let second = provider(store);
    let token = first
        .issue_session(SubjectId("subject-a".into()))
        .await
        .expect("session persists");
    let headers = headers_with_cookie(&format!("{SESSION_COOKIE}={token}"));

    assert_eq!(
        second.resolve(&headers).await.expect("session loads"),
        Some(SubjectId("subject-a".into()))
    );
    drop(first);
    assert_eq!(
        second
            .resolve(&headers)
            .await
            .expect("session survives restart"),
        Some(SubjectId("subject-a".into()))
    );
}

#[tokio::test]
async fn canonical_failure_falls_back_to_a_valid_legacy_cookie() {
    let auth = provider(Arc::new(MemoryAuthStore::default()));
    let legacy = auth
        .issue_session(SubjectId("legacy-subject".into()))
        .await
        .expect("session persists");
    let headers = headers_with_cookie(&format!(
        "{LEGACY_SESSION_COOKIE}={legacy}; {SESSION_COOKIE}=not-issued"
    ));
    assert_eq!(
        auth.resolve(&headers).await.expect("session loads"),
        Some(SubjectId("legacy-subject".into()))
    );
}

#[tokio::test]
async fn application_expiry_refuses_a_record_before_ttl_cleanup() {
    let store = Arc::new(MemoryAuthStore::default());
    let raw = "expired-cookie";
    store
        .put_browser_session(BrowserSession {
            id: SecretDigest::identify(BROWSER_SESSION_DOMAIN, raw),
            subject: SubjectId("subject-a".into()),
            expires_at: SystemTime::now() - Duration::from_secs(1),
        })
        .await
        .expect("expired fixture persists");
    let auth = provider(store);
    let headers = headers_with_cookie(&format!("{SESSION_COOKIE}={raw}"));
    assert_eq!(auth.resolve(&headers).await.expect("store works"), None);
}

#[tokio::test]
async fn a_cookie_store_outage_is_not_reported_as_unauthenticated() {
    let store = Arc::new(MemoryAuthStore::default());
    let auth = provider(store.clone());
    let token = auth
        .issue_session(SubjectId("subject-a".into()))
        .await
        .expect("session persists");
    store.set_unavailable(true);
    let headers = headers_with_cookie(&format!("{SESSION_COOKIE}={token}"));
    assert!(matches!(
        auth.resolve(&headers).await,
        Err(StoreError::Unavailable(_))
    ));
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
    let error = auth.metadata().await.expect_err("discovery must fail");
    assert!(error.contains("discovery request"), "got {error}");
}

#[test]
fn rebase_moves_only_the_origin_and_preserves_path_and_query() {
    assert_eq!(
        rebase(
            "https://public.example/oauth/userinfo?x=1",
            "http://provider.internal:3001/",
        ),
        "http://provider.internal:3001/oauth/userinfo?x=1"
    );
    assert_eq!(rebase("not-a-url", "http://internal:1"), "not-a-url");
}
