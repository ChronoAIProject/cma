use super::*;
use layer2_core::dependency_diagnostics::{capture, Dependency, Failure, Reason};

#[tokio::test]
async fn cookie_and_bearer_keep_final_reasons_but_not_successful_fallbacks() {
    for cookie in [false, true] {
        for (status, reason) in [
            (502, Reason::BadGateway),
            (503, Reason::UpstreamUnavailable),
            (429, Reason::InvalidEvidence),
        ] {
            let fixture = Fixture::new().await;
            let retained = fixture.seed().await;
            fixture.auth.metadata().await.unwrap();
            fixture.control.introspection.write().await.status =
                StatusCode::from_u16(status).unwrap();
            let (result, failure) = capture(async {
                if cookie {
                    request_for_browser_cookie(&fixture.auth, &retained.subject, retained.clone())
                        .await
                } else {
                    identity::request_for_bearer(&fixture.auth, "independent-fixture".into()).await
                }
            })
            .await;
            assert!(result.is_err());
            assert_eq!(
                failure,
                Some(Failure::new(Dependency::IdentityProvider, reason))
            );
            fixture.control.introspection.write().await.status = StatusCode::OK;
            let (result, failure) = capture(request_for_session(
                &fixture.auth,
                &retained.subject,
                retained.clone(),
            ))
            .await;
            assert!(result.unwrap().is_some());
            assert_eq!(failure, None);
            fixture.control.introspection.write().await.status = StatusCode::SERVICE_UNAVAILABLE;
            let (result, failure) = capture(request_for_session(
                &fixture.auth,
                &retained.subject,
                retained.clone(),
            ))
            .await;
            assert!(result.unwrap().is_some());
            assert_eq!(failure, None, "successful cached fallback is not a failure");
        }
    }
}

#[tokio::test]
async fn final_reason_allowlist_survives_cloning_without_arbitrary_labels() {
    for (error, reason) in [
        (
            ProviderVerificationError::Transient("timeout"),
            Reason::Timeout,
        ),
        (
            ProviderVerificationError::Transient("connection"),
            Reason::Connection,
        ),
        (
            ProviderVerificationError::Transient("deadline"),
            Reason::VerificationDeadline,
        ),
        (
            ProviderVerificationError::Transient("private fixture label"),
            Reason::Unknown,
        ),
        (
            ProviderVerificationError::Unavailable("private fixture detail".into()),
            Reason::InvalidEvidence,
        ),
    ] {
        let cloned = error.clone();
        let (_, failure) = capture(async {
            cloned.record_unavailable();
        })
        .await;
        assert_eq!(
            failure,
            Some(Failure::new(Dependency::IdentityProvider, reason))
        );
    }
}

#[tokio::test]
async fn authentication_logs_one_content_free_counter_per_503_only() {
    use axum::{
        body::{to_bytes, Body},
        http::Request,
    };
    use tower::ServiceExt as _;
    use tracing::instrument::WithSubscriber;
    #[derive(Clone)]
    struct Writer(Arc<std::sync::Mutex<Vec<u8>>>);
    impl std::io::Write for Writer {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let bytes = Arc::new(std::sync::Mutex::new(Vec::new()));
    let writer = Writer(bytes.clone());
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_writer(move || writer.clone())
        .finish();
    async {
        let f = Fixture::new().await;
        f.seed().await;
        f.auth.metadata().await.unwrap();
        let app = crate::auth::router(f.auth.clone()).layer(axum::middleware::from_fn_with_state(
            f.auth.clone(),
            crate::auth::attach_subject,
        ));
        for (upstream, expected, code) in [
            (429, 503, Some("store_failed")),
            (503, 503, Some("identity_unavailable")),
            (200, 200, None),
            (401, 401, None),
        ] {
            f.control.introspection.write().await.status = StatusCode::from_u16(upstream).unwrap();
            let request = Request::get("/auth/session")
                .header(header::AUTHORIZATION, "Bearer independent-fixture-token")
                .body(Body::empty())
                .unwrap();
            let response = app.clone().oneshot(request).await.unwrap();
            assert_eq!(response.status().as_u16(), expected);
            let body: Value =
                serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap())
                    .unwrap();
            if let Some(code) = code {
                assert_eq!(body["code"], code);
            }
        }
    }
    .with_subscriber(subscriber)
    .await;
    let text = String::from_utf8(bytes.lock().unwrap().clone()).unwrap();
    let events: Vec<_> = text
        .lines()
        .filter(|line| line.contains("cma_dependency_failures_total"))
        .collect();
    assert_eq!(events.len(), 2, "{text}");
    assert!(events[0].contains("invalid_evidence"));
    assert!(events[1].contains("http_503"));
    for line in events {
        assert!(line.contains("authentication"));
        assert!(line.contains("identity_provider"));
        for private in ["fixture", "token", "owner", "http://", "path=", "subject="] {
            assert!(!line.contains(private), "{line}");
        }
    }
}
