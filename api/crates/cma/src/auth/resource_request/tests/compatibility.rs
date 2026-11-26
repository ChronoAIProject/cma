//! Legacy identity propagation must never become either authentication or a refusal.

use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

const LEGACY_HEADER: &str = "x-nyxid-identity-token";

async fn inspect_ambient(auth: layer1_public::AuthenticatedResourceCaller) -> Json<Value> {
    assert!(matches!(
        auth.context.credential_source,
        CredentialSource::Independent { .. }
    ));
    assert!(auth.credential.is_some());
    Json(json!({"subject": auth.context.caller.subject.as_str()}))
}

#[tokio::test]
async fn legacy_identity_header_preserves_independently_verified_bearers_and_keys() {
    let userinfo_calls = Arc::new(AtomicUsize::new(0));
    let seen_userinfo = userinfo_calls.clone();
    let forms = Arc::new(RwLock::new(Vec::<HashMap<String, String>>::new()));
    let seen_forms = forms.clone();
    let bearer = uuid::Uuid::new_v4().to_string();
    let key = format!("nyx_{}", uuid::Uuid::new_v4().simple());
    let expected_bearer = bearer.clone();
    let expiry = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 3600;
    let provider = Router::new()
        .route(
            "/userinfo",
            get(move |headers: HeaderMap| {
                let seen = seen_userinfo.clone();
                let expected = expected_bearer.clone();
                async move {
                    assert_eq!(headers[header::AUTHORIZATION], format!("Bearer {expected}"));
                    seen.fetch_add(1, Ordering::SeqCst);
                    Json(json!({"sub": "owner"}))
                }
            }),
        )
        .route(
            "/introspect",
            post(move |Form(form): Form<HashMap<String, String>>| {
                let seen = seen_forms.clone();
                async move {
                    let hint = form["token_type_hint"].clone();
                    seen.write().await.push(form);
                    Json(match hint.as_str() {
                        "access_token" => json!({
                            "active": true, "sub": "owner", "token_type": "access",
                            "exp": expiry, "permissions": ["cma:sandbox:use"]
                        }),
                        "api_key" => json!({
                            "active": true, "sub": "owner", "token_type": "api_key",
                            "aud": "cma-service", "scope": "proxy",
                            "permissions": ["cma:sandbox:use"]
                        }),
                        "resource_request" => json!({"active": false}),
                        _ => panic!("unexpected introspection type"),
                    })
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, provider).await.unwrap() });
    for enabled in [false, true] {
        let store = Arc::new(MemoryAuthStore::default());
        store.set_unavailable(true);
        let auth = AuthState::with_store(
            AuthMode::Provider {
                base_url: base.clone(),
                client_id: "cma-client".into(),
                client_secret: uuid::Uuid::new_v4().to_string(),
                public_url: "https://cma.example".into(),
            },
            None,
            store,
        )
        .with_agent_key_service(Some("cma-service".into()))
        .with_resource_request_enabled(enabled);
        *auth.fixture_metadata().write().await = Some(ProviderMetadata {
            authorization_endpoint: format!("{base}/authorize"),
            token_endpoint: format!("{base}/token"),
            introspection_endpoint: Some(format!("{base}/introspect")),
            userinfo_endpoint: Some(format!("{base}/userinfo")),
        });
        let app = Router::new()
            .route("/api/v1/agents", get(inspect_ambient))
            .layer(axum::middleware::from_fn_with_state(
                auth,
                crate::auth::attach_subject,
            ));
        for credential in [&bearer, &key] {
            let read = HttpRequest::get("/api/v1/agents")
                .header(header::AUTHORIZATION, format!("Bearer {credential}"))
                .header(LEGACY_HEADER, "untrusted-legacy-identity-assertion")
                .body(Body::empty())
                .unwrap();
            let response = app.clone().oneshot(read).await.unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let body = to_bytes(response.into_body(), 4096).await.unwrap();
            assert_eq!(
                serde_json::from_slice::<Value>(&body).unwrap(),
                json!({"subject":"owner"})
            );
        }
        let legacy_only = HttpRequest::get("/api/v1/agents")
            .header(LEGACY_HEADER, "untrusted-legacy-identity-assertion")
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            app.clone().oneshot(legacy_only).await.unwrap().status(),
            StatusCode::UNAUTHORIZED
        );
        let before_userinfo = userinfo_calls.load(Ordering::SeqCst);
        let before_forms = forms.read().await.len();
        for proof in ["", "malformed-explicit-proof"] {
            let explicit = HttpRequest::get("/api/v1/agents")
                .header(header::AUTHORIZATION, format!("Bearer {bearer}"))
                .header(LEGACY_HEADER, "untrusted-legacy-identity-assertion")
                .header(HEADER, proof)
                .body(Body::empty())
                .unwrap();
            assert_eq!(
                app.clone().oneshot(explicit).await.unwrap().status(),
                if enabled {
                    StatusCode::UNAUTHORIZED
                } else {
                    StatusCode::FORBIDDEN
                }
            );
        }
        assert_eq!(userinfo_calls.load(Ordering::SeqCst), before_userinfo);
        let forms = forms.read().await;
        assert_eq!(forms.len(), before_forms + usize::from(enabled));
        assert!(forms[before_forms..]
            .iter()
            .all(|form| form["token_type_hint"] == "resource_request"));
    }
    assert_eq!(userinfo_calls.load(Ordering::SeqCst), 2);
    for hint in ["access_token", "api_key"] {
        assert_eq!(
            forms
                .read()
                .await
                .iter()
                .filter(|form| form["token_type_hint"] == hint)
                .count(),
            2
        );
    }
    server.abort();
}
