use super::*;
use crate::auth::persistence::{test_observer::ObservedAuthStore, MemoryAuthStore};
use axum::http::{header, HeaderMap};

fn headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::COOKIE,
        "cma_session=fixture-cookie".parse().unwrap(),
    );
    headers
}
fn login(expired: bool) -> BrowserLogin {
    BrowserLogin {
        id: SecretDigest::identify(AUTH_COOKIE_DOMAIN, "fixture-cookie"),
        subject: SubjectId::new("fixture-owner"),
        expires_at: if expired {
            SystemTime::UNIX_EPOCH
        } else {
            SystemTime::now() + Duration::from_secs(600)
        },
    }
}
fn auth(store: Arc<dyn AuthStore>, joined: bool) -> Arc<AuthState> {
    AuthState::with_store(
        AuthMode::Provider {
            base_url: "http://127.0.0.1:1".into(),
            client_id: "fixture".into(),
            client_secret: "fixture-secret".into(),
            public_url: "https://example.test".into(),
        },
        None,
        store,
    )
    .with_browser_session_join(joined)
}

#[tokio::test]
async fn both_readers_preserve_absence_expiry_missing_subject_and_store_failure() {
    for joined in [false, true] {
        let store = Arc::new(MemoryAuthStore::default());
        let observed = Arc::new(ObservedAuthStore::new(store.clone(), Duration::ZERO));
        let auth = auth(observed.clone(), joined);
        assert!(auth.resolve_request(&headers()).await.unwrap().is_none());
        store.put_browser_login(login(true)).await.unwrap();
        assert!(auth.resolve_request(&headers()).await.unwrap().is_none());
        assert!(store
            .browser_login(&login(true).id)
            .await
            .unwrap()
            .is_none());
        store.put_browser_login(login(false)).await.unwrap();
        let resolved = auth.resolve_request(&headers()).await.unwrap().unwrap();
        assert_eq!(resolved.subject, login(false).subject);
        assert_eq!(resolved.authorization, Authorization::none());
        assert!(resolved.delegated_credential.is_none());
        store.set_unavailable(true);
        assert!(matches!(
            auth.resolve_request(&headers()).await,
            Err(StoreError::Unavailable(_))
        ));
        let reads = observed.reads.lock().unwrap();
        let names: Vec<_> = reads.iter().map(|(name, _)| *name).collect();
        assert_eq!(
            names,
            if joined {
                vec!["join", "join", "join", "join"]
            } else {
                vec!["login", "login", "login", "subject", "login"]
            }
        );
    }
}

#[tokio::test]
async fn both_readers_reject_crossed_subject_before_provider_work() {
    for joined in [false, true] {
        let store = Arc::new(MemoryAuthStore::default());
        store.put_browser_login(login(false)).await.unwrap();
        store
            .insert_subject(SubjectAuthState::new(
                login(false).subject,
                Authorization::none(),
                Profile::default(),
                None,
            ))
            .await
            .unwrap();
        let mut observed = ObservedAuthStore::new(store, Duration::ZERO);
        observed.wrong_subject = true;
        let auth = auth(Arc::new(observed), joined);
        assert!(matches!(
            auth.resolve_request(&headers()).await,
            Err(StoreError::Malformed(_))
        ));
    }
}

#[tokio::test]
async fn identity_phase_is_scoped_to_resolution_and_keeps_request_labels() {
    use axum::{body::Body, http::Request, routing::get, Router};
    use layer2_core::diagnostic_context::DiagnosticContext;
    use tower::ServiceExt;
    for joined in [false, true] {
        let store = Arc::new(MemoryAuthStore::default());
        store.put_browser_login(login(false)).await.unwrap();
        let observed = Arc::new(ObservedAuthStore::new(store, Duration::ZERO));
        let app = Router::new()
            .route(
                "/probe",
                get(|| async {
                    assert_eq!(DiagnosticContext::current().phase, "handler");
                    "ok"
                }),
            )
            .layer(axum::middleware::from_fn_with_state(
                auth(observed.clone(), joined),
                middleware::attach_subject,
            ));
        let context = DiagnosticContext::current().with_phase("handler");
        let response = context
            .scope(
                app.oneshot(
                    Request::get("/probe")
                        .header(header::COOKIE, "cma_session=fixture-cookie")
                        .body(Body::empty())
                        .unwrap(),
                ),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        assert!(observed
            .reads
            .lock()
            .unwrap()
            .iter()
            .all(|(_, phase)| *phase == "identity"));
    }
}
