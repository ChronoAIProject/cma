use super::*;
use axum::{extract::State, routing::get, Router};
use std::sync::{atomic::AtomicUsize, Arc};

fn headers(token: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert("authorization", format!("Bearer {token}").parse().unwrap());
    headers
}

#[tokio::test]
async fn authentication_caches_hashes_expires_and_bounds_upstream_responses() {
    let calls = Arc::new(AtomicUsize::new(0));
    let router = Router::new()
        .route(
            "/api/v1/users/me",
            get(
                |State(calls): State<Arc<AtomicUsize>>, headers: HeaderMap| async move {
                    calls.fetch_add(1, Ordering::Relaxed);
                    match headers.get("authorization").unwrap().to_str().unwrap() {
                        "Bearer valid-fixture" => (StatusCode::OK, r#"{"id":"alice"}"#.to_owned()),
                        "Bearer down" => (StatusCode::SERVICE_UNAVAILABLE, String::new()),
                        "Bearer large" => (StatusCode::OK, "x".repeat(65_537)),
                        _ => (StatusCode::UNAUTHORIZED, String::new()),
                    }
                },
            ),
        )
        .with_state(calls.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let auth =
        CreatorAuthenticator::new(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    assert_eq!(
        auth.authenticate(&HeaderMap::new()).await,
        Err(CreatorAuthError::NotAuthenticated)
    );
    let first = auth.authenticate(&headers("valid-fixture")).await.unwrap();
    assert_eq!(first.subject, "alice");
    assert_eq!(first.token_hash, token_hash("valid-fixture"));
    assert!(!format!("{auth:?}").contains("valid-fixture"));
    assert_eq!(
        auth.authenticate(&headers("valid-fixture")).await.unwrap(),
        first
    );
    assert_eq!(calls.load(Ordering::Relaxed), 1);
    for _ in 0..2 {
        assert_eq!(
            auth.authenticate(&headers("invalid")).await,
            Err(CreatorAuthError::NotAuthenticated)
        );
    }
    assert_eq!(calls.load(Ordering::Relaxed), 2);
    let positive_ttl = auth.cache.get(&first.token_hash).unwrap().expires_at - Instant::now();
    let negative_ttl = auth.cache.get(&token_hash("invalid")).unwrap().expires_at - Instant::now();
    assert!(positive_ttl > Duration::from_secs(50) && positive_ttl <= SUCCESS_TTL);
    assert!(negative_ttl > Duration::from_secs(5) && negative_ttl <= FAILURE_TTL);
    auth.cache.get_mut(&first.token_hash).unwrap().expires_at = Instant::now();
    auth.authenticate(&headers("valid-fixture")).await.unwrap();
    assert_eq!(calls.load(Ordering::Relaxed), 3);
    for token in ["down", "large"] {
        assert_eq!(
            auth.authenticate(&headers(token)).await,
            Err(CreatorAuthError::ProviderUnavailable)
        );
    }
    task.abort();
    task.await.unwrap_err();
    // A successful cached subject survives an outage; an uncached bearer fails closed.
    assert_eq!(
        auth.authenticate(&headers("valid-fixture")).await.unwrap(),
        first
    );
    assert_eq!(
        auth.authenticate(&headers("uncached")).await,
        Err(CreatorAuthError::ProviderUnavailable)
    );
}

#[test]
fn cache_evicts_oldest_at_capacity() {
    let auth = CreatorAuthenticator::new("http://127.0.0.1:1").unwrap();
    for sequence in 0..=CACHE_LIMIT {
        auth.insert_cache(
            sequence.to_string(),
            CacheEntry {
                result: Ok(("alice".into(), false)),
                expires_at: Instant::now() + SUCCESS_TTL,
                sequence: sequence as u64,
            },
        );
    }
    assert_eq!(auth.cache.len(), CACHE_LIMIT);
    assert!(!auth.cache.contains_key("0"));
    assert!(auth.cache.contains_key(&CACHE_LIMIT.to_string()));
}

#[tokio::test]
async fn organization_type_requires_provider_evidence_and_survives_auth_cache() {
    let router = Router::new()
        .route(
            "/api/v1/users/me",
            get(|headers: HeaderMap| async move {
                match headers.get("authorization").unwrap().to_str().unwrap() {
                    "Bearer organization" => r#"{"id":"org","user_type":"org"}"#,
                    "Bearer legacy-org" => r#"{"id":"org"}"#,
                    "Bearer human" => r#"{"id":"person","user_type":"person"}"#,
                    "Bearer legacy-human" => r#"{"id":"person"}"#,
                    _ => r#"{"id":"wrong-proof"}"#,
                }
            }),
        )
        .route(
            "/api/v1/orgs/org/authorization",
            get(|| async { r#"{"id":"org","your_role":"admin"}"# }),
        )
        .route(
            "/api/v1/orgs/wrong-proof/authorization",
            get(|| async { r#"{"id":"another-org","your_role":"admin"}"# }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let auth =
        CreatorAuthenticator::new(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    for token in ["organization", "legacy-org"] {
        for _ in 0..2 {
            assert!(
                auth.authenticate(&headers(token))
                    .await
                    .unwrap()
                    .organization
            );
        }
    }
    for token in ["human", "legacy-human"] {
        assert!(
            !auth
                .authenticate(&headers(token))
                .await
                .unwrap()
                .organization
        );
    }
    assert_eq!(
        auth.authenticate(&headers("wrong")).await,
        Err(CreatorAuthError::NotAuthenticated)
    );
    task.abort();
}

#[tokio::test]
async fn organization_key_uses_userinfo_only_with_verified_organization_proof() {
    let router = Router::new()
        .route("/api/v1/users/me", get(|| async { StatusCode::FORBIDDEN }))
        .route(
            "/oauth/userinfo",
            get(|headers: HeaderMap| async move {
                match headers.get("authorization").unwrap().to_str().unwrap() {
                    "Bearer org-key" => (StatusCode::OK, r#"{"sub":"org"}"#),
                    "Bearer forbidden-person" => (StatusCode::OK, r#"{"sub":"person"}"#),
                    "Bearer wrong-proof" => (StatusCode::OK, r#"{"sub":"wrong-proof"}"#),
                    _ => (StatusCode::UNAUTHORIZED, ""),
                }
            }),
        )
        .route(
            "/api/v1/orgs/org/authorization",
            get(|| async { r#"{"id":"org","your_role":"admin"}"# }),
        )
        .route(
            "/api/v1/orgs/wrong-proof/authorization",
            get(|| async { r#"{"id":"org","your_role":"admin"}"# }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let auth =
        CreatorAuthenticator::new(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let creator = auth.authenticate(&headers("org-key")).await.unwrap();
    assert_eq!(creator.subject, "org");
    assert!(creator.organization);
    for token in ["forbidden-person", "wrong-proof", "invalid"] {
        assert_eq!(
            auth.authenticate(&headers(token)).await,
            Err(CreatorAuthError::NotAuthenticated)
        );
    }
    task.abort();
}
