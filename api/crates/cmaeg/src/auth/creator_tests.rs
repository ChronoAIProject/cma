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

#[tokio::test]
async fn selected_organization_requires_this_bearers_admin_proof_and_scopes_cache() {
    use axum::extract::Path;
    let calls = Arc::new(AtomicUsize::new(0));
    let revoked = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let router = Router::new()
        .route(
            "/api/v1/users/me",
            get(|headers: HeaderMap| async move {
                if headers.get("authorization").unwrap() == "Bearer organization" {
                    r#"{"id":"org-a","user_type":"org"}"#
                } else {
                    r#"{"id":"human","user_type":"person"}"#
                }
            }),
        )
        .route(
            "/api/v1/orgs/{owner}/authorization",
            get({
                let calls = calls.clone();
                let revoked = revoked.clone();
                move |Path(owner): Path<String>, headers: HeaderMap| {
                    let calls = calls.clone();
                    let revoked = revoked.clone();
                    async move {
                        calls.fetch_add(1, Ordering::Relaxed);
                        let authorized = headers.get("authorization").unwrap() == "Bearer human";
                        let (id, role) = match owner.as_str() {
                            "org-a" if authorized && !revoked.load(Ordering::Relaxed) => {
                                ("org-a", "admin")
                            }
                            "org-a" => ("org-a", "viewer"),
                            "org-b" => ("org-b", "member"),
                            "wrong-proof" => ("org-a", "admin"),
                            _ => return (StatusCode::NOT_FOUND, String::new()),
                        };
                        (
                            StatusCode::OK,
                            serde_json::json!({"id": id, "your_role":role}).to_string(),
                        )
                    }
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let auth =
        CreatorAuthenticator::new(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let select = |token: &str, owner: &str| {
        let mut value = headers(token);
        value.insert("x-cmaeg-owner", owner.parse().unwrap());
        value
    };
    let personal = auth.authenticate(&headers("human")).await.unwrap();
    assert_eq!(personal.subject, "human");
    assert!(!personal.organization);
    let organization = auth.authenticate(&select("human", "org-a")).await.unwrap();
    assert_eq!(organization.subject, "org-a");
    assert!(organization.organization);
    assert_eq!(organization.token_hash, personal.token_hash);
    assert_eq!(
        auth.authenticate(&select("human", "org-a")).await.unwrap(),
        organization
    );
    assert_eq!(calls.load(Ordering::Relaxed), 1);
    for (token, owner) in [
        ("human", "org-b"),
        ("human", "wrong-proof"),
        ("human", "unknown"),
        ("different", "org-a"),
        ("organization", "org-b"),
    ] {
        assert_eq!(
            auth.authenticate(&select(token, owner)).await,
            Err(CreatorAuthError::NotAuthenticated)
        );
    }
    assert_eq!(
        auth.authenticate(&headers("human")).await.unwrap(),
        personal
    );
    assert_eq!(
        auth.authenticate(&select("human", "org-a")).await.unwrap(),
        organization
    );
    let own_organization = auth
        .authenticate(&select("organization", "org-a"))
        .await
        .unwrap();
    assert_eq!(own_organization.subject, "org-a");
    revoked.store(true, Ordering::Relaxed);
    let key = format!("{}:org-a", token_hash("human"));
    auth.cache.get_mut(&key).unwrap().expires_at = Instant::now();
    assert_eq!(
        auth.authenticate(&select("human", "org-a")).await,
        Err(CreatorAuthError::NotAuthenticated)
    );
    task.abort();
}

#[tokio::test]
async fn malformed_or_duplicate_owner_selection_fails_before_upstream() {
    let auth = CreatorAuthenticator::new("http://127.0.0.1:1").unwrap();
    for value in ["", "two owners", " leading", "trailing "] {
        let mut input = headers("fixture");
        input.insert("x-cmaeg-owner", value.parse().unwrap());
        assert_eq!(
            auth.authenticate(&input).await,
            Err(CreatorAuthError::NotAuthenticated)
        );
    }
    let mut input = headers("fixture");
    input.append("x-cmaeg-owner", "first".parse().unwrap());
    input.append("x-cmaeg-owner", "second".parse().unwrap());
    assert_eq!(
        auth.authenticate(&input).await,
        Err(CreatorAuthError::NotAuthenticated)
    );
}
