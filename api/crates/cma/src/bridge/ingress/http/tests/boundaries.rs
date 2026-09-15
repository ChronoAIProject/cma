use super::*;

#[tokio::test]
async fn callback_route_bypasses_only_browser_middleware_and_keeps_management_authentication() {
    let fixture = HttpFixture::new(false);
    fixture.provider.commits.add_permits(1);
    let browser_calls = Arc::new(AtomicUsize::new(0));
    let calls = browser_calls.clone();
    let public = crate::rest_router(layer1_public::rest::bridge_router(None)).layer(
        axum::middleware::from_fn(
            move |request: axum::extract::Request, next: axum::middleware::Next| {
                let calls = calls.clone();
                async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    next.run(request).await
                }
            },
        ),
    );
    let app = Router::new().merge(public).merge(fixture.router());
    let response = app.clone().oneshot(fixture.request()).await.unwrap();
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    assert_eq!(browser_calls.load(Ordering::SeqCst), 0);
    let mut management = fixture.request();
    *management.uri_mut() = "/api/v1/bridges".parse().unwrap();
    *management.method_mut() = axum::http::Method::GET;
    let response = app.oneshot(management).await.unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(browser_calls.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.provider.inbox_calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn malformed_headers_and_declared_oversize_fail_before_body_or_provider_work() {
    for bad in ["duplicate", "oversize", "content_type"] {
        let fixture = HttpFixture::new(false);
        let read = Arc::new(AtomicUsize::new(0));
        let read_count = read.clone();
        let body = Body::from_stream(futures_util::stream::once(async move {
            read_count.fetch_add(1, Ordering::SeqCst);
            Ok::<_, std::io::Error>(axum::body::Bytes::from_static(b"must-not-read"))
        }));
        let mut req = fixture.request();
        *req.body_mut() = body;
        let expected = match bad {
            "duplicate" => {
                req.headers_mut()
                    .append("x-nyxid-message-id", "other".parse().unwrap());
                StatusCode::UNAUTHORIZED
            }
            "oversize" => {
                req.headers_mut().insert(
                    header::CONTENT_LENGTH,
                    (MAX_BODY + 1).to_string().parse().unwrap(),
                );
                StatusCode::PAYLOAD_TOO_LARGE
            }
            _ => {
                req.headers_mut()
                    .insert(header::CONTENT_TYPE, "text/plain".parse().unwrap());
                StatusCode::UNSUPPORTED_MEDIA_TYPE
            }
        };
        let response = fixture.router().oneshot(req).await.unwrap();
        assert_eq!(response.status(), expected);
        assert_eq!(read.load(Ordering::SeqCst), 0);
        assert_eq!(fixture.provider.provider_calls.load(Ordering::SeqCst), 0);
        assert_eq!(fixture.provider.inbox_calls.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn chunked_oversize_and_tampered_signature_never_reach_live_provider_or_inbox() {
    let fixture = HttpFixture::new(false);
    for oversized in [true, false] {
        let mut req = fixture.request();
        *req.body_mut() = if oversized {
            Body::from(vec![b'a'; MAX_BODY + 1])
        } else {
            Body::from(b"unsigned bytes".to_vec())
        };
        let response = fixture.router().oneshot(req).await.unwrap();
        assert_eq!(
            response.status(),
            if oversized {
                StatusCode::PAYLOAD_TOO_LARGE
            } else {
                StatusCode::UNAUTHORIZED
            }
        );
    }
    assert_eq!(fixture.provider.provider_calls.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.provider.inbox_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test(start_paused = true)]
async fn stalled_body_expires_before_ingress_authentication_or_storage() {
    let fixture = HttpFixture::new(false);
    let mut req = fixture.request();
    *req.body_mut() = Body::from_stream(futures_util::stream::pending::<
        Result<axum::body::Bytes, std::io::Error>,
    >());
    let response = fixture.router().oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::REQUEST_TIMEOUT);
    assert_eq!(value(response).await["code"], "callback_body_timeout");
    assert_eq!(fixture.provider.provider_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn unavailable_ingress_has_an_explicit_response_and_no_successful_ack() {
    let fixture = HttpFixture::new(false);
    let response = router(None).oneshot(fixture.request()).await.unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        value(response).await["code"],
        "callback_ingress_unavailable"
    );
    fixture.provider.failure.store(1, Ordering::SeqCst);
    fixture.provider.commits.add_permits(1);
    let response = fixture.router().oneshot(fixture.request()).await.unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(fixture.provider.rows.lock().await.is_empty());
}
