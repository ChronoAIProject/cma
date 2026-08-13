//! Tests for establishing who is calling.
//!
//! In a sibling file so `mod.rs` stays within the project's file-size limit
//! while what it guarantees stays covered.

use super::*;

fn headers_with_cookie(value: &str) -> HeaderMap {
    let mut h = HeaderMap::new();
    h.insert(header::COOKIE, value.parse().expect("valid header"));
    h
}

#[test]
fn the_session_cookie_is_found_among_others() {
    let h = headers_with_cookie("theme=dark; chrono_bot_session=abc123; other=1");
    assert_eq!(session_cookie(&h).as_deref(), Some("abc123"));
}

#[test]
fn a_missing_session_cookie_is_none() {
    let h = headers_with_cookie("theme=dark; other=1");
    assert_eq!(session_cookie(&h), None);
    assert_eq!(session_cookie(&HeaderMap::new()), None);
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
