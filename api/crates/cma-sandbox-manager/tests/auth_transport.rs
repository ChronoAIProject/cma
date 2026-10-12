use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};

use axum::{
    http::{HeaderMap, StatusCode},
    routing::post,
    Form, Json, Router,
};
use cma_sandbox_manager::{
    auth::{AuthError, NyxIdAuth},
    permissions,
};

#[tokio::test]
async fn authorization_observes_revocation_and_uses_this_services_client() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let issuer = format!("http://{}", listener.local_addr().unwrap());
    let resource = format!("{issuer}/api/v1/proxy/s/cma-sandbox-manager");
    let auth = NyxIdAuth::new(&issuer, &resource, "sandbox-client", "unused".into()).unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let observations = calls.clone();
    let app = Router::new().route("/oauth/introspect", post(move |Form(form): Form<HashMap<String, String>>| {
        let calls = calls.clone(); let issuer = issuer.clone(); let resource = resource.clone();
        async move {
            if form.get("client_id").map(String::as_str) != Some("sandbox-client")
                || form.get("client_secret").map(String::as_str) != Some("unused")
                || form.get("token_type_hint").map(String::as_str) != Some("access_token")
                || form.get("token").map(String::as_str) != Some("caller")
            { return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({}))); }
            if calls.fetch_add(1, Ordering::SeqCst) > 0 {
                return (StatusCode::OK, Json(serde_json::json!({"active":false})));
            }
            let exp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() + 60;
            (StatusCode::OK, Json(serde_json::json!({
                "active": true, "sub": "alice", "iss": issuer, "exp": exp, "token_type": "access",
                "resource": [resource], "permissions": [permissions::CREATE]
            })))
        }
    }));
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let mut headers = HeaderMap::new();
    headers.insert("authorization", "Bearer caller".parse().unwrap());
    let principal = auth.authenticate(&headers).await.unwrap();
    assert_eq!(principal.subject, "alice");
    principal.require(permissions::CREATE).unwrap();
    assert_eq!(
        principal.require(permissions::DELETE),
        Err(AuthError::Forbidden)
    );
    assert!(matches!(
        auth.authenticate(&headers).await,
        Err(AuthError::Unauthenticated)
    ));
    assert_eq!(observations.load(Ordering::SeqCst), 2);
    headers.append("authorization", "Bearer second".parse().unwrap());
    assert!(matches!(
        auth.authenticate(&headers).await,
        Err(AuthError::Unauthenticated)
    ));
    assert_eq!(observations.load(Ordering::SeqCst), 2);
    server.abort();
}

#[tokio::test]
async fn introspection_redirects_never_forward_client_credentials() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let issuer = format!("http://{}", listener.local_addr().unwrap());
    let auth = NyxIdAuth::new(
        &issuer,
        &format!("{issuer}/api/v1/proxy/s/cma-sandbox-manager"),
        "sandbox-client",
        "unused".into(),
    )
    .unwrap();
    let redirects = Arc::new(AtomicUsize::new(0));
    let observations = redirects.clone();
    let app = Router::new()
        .route(
            "/oauth/introspect",
            post(|| async { (StatusCode::TEMPORARY_REDIRECT, [("location", "/capture")]) }),
        )
        .route(
            "/capture",
            post(move || {
                let redirects = redirects.clone();
                async move {
                    redirects.fetch_add(1, Ordering::SeqCst);
                    StatusCode::OK
                }
            }),
        );
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let mut headers = HeaderMap::new();
    headers.insert("authorization", "Bearer caller".parse().unwrap());
    assert!(matches!(
        auth.authenticate(&headers).await,
        Err(AuthError::Unavailable)
    ));
    assert_eq!(observations.load(Ordering::SeqCst), 0);
    server.abort();
}
