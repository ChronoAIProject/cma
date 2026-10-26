use super::*;
use futures_util::StreamExt;
use std::convert::Infallible;

#[test]
fn node_configuration_and_delivery_controls_use_only_their_exact_public_operations() {
    for (path, methods) in [
        ("/v1/channel-capabilities", vec![Method::GET]),
        ("/v1/channel-connections", vec![Method::GET]),
        ("/v1/channels/channel/routing", vec![Method::PUT]),
        ("/v1/channels/channel/outbound-messages", vec![Method::POST]),
        (
            "/v1/channels/channel/outbound-messages/delivery",
            vec![Method::GET, Method::DELETE],
        ),
        (
            "/v1/channels/channel/outbound-messages/delivery/send",
            vec![Method::POST],
        ),
    ] {
        for method in [
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::PATCH,
            Method::DELETE,
        ] {
            assert_eq!(
                allowed(&method, path),
                methods.contains(&method),
                "{method} {path}"
            );
            assert!(!allowed(&method, &format!("{path}/private/unlisted")));
        }
    }
    assert!(!allowed(
        &Method::POST,
        "/v1/channels/channel/outbound-messages/../send"
    ));
    assert!(!allowed(
        &Method::POST,
        "/v1/channel-connections/connection/test"
    ));
}

pub(super) fn routes(auth: cma_service_auth::NyxIdAuth) -> Router<Arc<Control>> {
    Router::new().route(
        "/v1/channels/channel/activity/stream",
        get(move |State(control): State<Arc<Control>>, headers: HeaderMap| {
            let auth = auth.clone();
            async move {
                control.calls.fetch_add(1, Ordering::SeqCst);
                *control.headers.write().await = headers.clone();
                if let Err(error) = auth.authenticate(&headers).await
                    .and_then(|principal| principal.require("cma-eg:channels:read")) {
                    return error.into_response();
                }
                if *control.redirect.read().await {
                    return Json(json!({"items":[]})).into_response();
                }
                let gate = control.gate.read().await.clone();
                let stream = futures_util::stream::unfold((gate, 0), |(gate, index)| async move {
                    if index == 2 { return None; }
                    if index == 1 {
                        if let Some(gate) = &gate {
                            gate.reached.notify_one();
                            gate.release.notified().await;
                        }
                    }
                    let frame = format!("id: cursor-{index}\nevent: activity\ndata: {{\"sequence\":{index}}}\n\n");
                    Some((Ok::<_, Infallible>(frame), (gate, index + 1)))
                });
                ([(header::CONTENT_TYPE, "text/event-stream"),
                    (header::SET_COOKIE, "must-not-cross=1")], Body::from_stream(stream)).into_response()
            }
        }),
    )
}

#[tokio::test]
async fn activity_arrives_before_upstream_completion_and_retains_reconnect_cursor() {
    let f = Fixture::new().await;
    let gate = Arc::new(Gate::default());
    *f.control.gate.write().await = Some(gate.clone());
    let mut request = f.request(Method::GET, "/v1/channels/channel/activity/stream");
    request
        .headers_mut()
        .insert("last-event-id", HeaderValue::from_static("prior-cursor"));
    let response = tokio::time::timeout(Duration::from_secs(2), f.send(request))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["x-accel-buffering"], "no");
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    assert!(!response.headers().contains_key(header::SET_COOKIE));
    let mut body = response.into_body().into_data_stream();
    let first = tokio::time::timeout(Duration::from_secs(2), body.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(String::from_utf8_lossy(&first).contains("cursor-0"));
    assert!(!String::from_utf8_lossy(&first).contains("cursor-1"));
    let headers = f.control.headers.read().await;
    assert_eq!(headers["last-event-id"], "prior-cursor");
    assert!(!headers.contains_key(header::COOKIE));
    drop(headers);
    gate.release.notify_one();
    let second = tokio::time::timeout(Duration::from_secs(2), body.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(String::from_utf8_lossy(&second).contains("cursor-1"));
    assert!(body.next().await.is_none());
}

#[tokio::test]
async fn activity_stream_stops_before_releasing_data_after_logout() {
    let f = Fixture::new().await;
    let gate = Arc::new(Gate::default());
    *f.control.gate.write().await = Some(gate.clone());
    let response = f
        .send(f.request(Method::GET, "/v1/channels/channel/activity/stream"))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let mut body = response.into_body().into_data_stream();
    assert!(body.next().await.unwrap().is_ok());
    f.auth.revoke_browser_login(&f.cookie).await.unwrap();
    gate.release.notify_one();
    assert!(tokio::time::timeout(Duration::from_secs(2), body.next())
        .await
        .unwrap()
        .unwrap()
        .is_err());
}

#[tokio::test]
async fn activity_requires_read_permission_and_does_not_accept_non_stream_success() {
    let f = Fixture::new().await;
    *f.control.permissions.write().await = vec!["cma:admin".into()];
    assert_eq!(
        f.send(f.request(Method::GET, "/v1/channels/channel/activity/stream"))
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    *f.control.permissions.write().await = vec!["cma-eg:channels:read".into()];
    *f.control.redirect.write().await = true;
    assert_eq!(
        f.send(f.request(Method::GET, "/v1/channels/channel/activity/stream"))
            .await
            .status(),
        StatusCode::BAD_GATEWAY
    );
}

#[tokio::test]
async fn activity_routes_and_cursors_do_not_widen_the_portal_boundary() {
    for path in [
        "/v1/channels/channel/activity",
        "/v1/channels/channel/activity/stream",
    ] {
        assert!(allowed(&Method::GET, path));
        for method in [Method::POST, Method::PUT, Method::DELETE] {
            assert!(!allowed(&method, path));
        }
    }
    let f = Fixture::new().await;
    for path in [
        "/v1/channels/channel/activity/private",
        "/v1/channels/channel/activity/stream/more",
    ] {
        assert_eq!(
            f.send(f.request(Method::GET, path)).await.status(),
            StatusCode::NOT_FOUND
        );
    }
    let mut request = f.request(Method::GET, "/v1/channels/channel/activity/stream");
    request
        .headers_mut()
        .append("last-event-id", HeaderValue::from_static("one"));
    request
        .headers_mut()
        .append("last-event-id", HeaderValue::from_static("two"));
    assert_eq!(f.send(request).await.status(), StatusCode::BAD_REQUEST);
    let mut request = f.request(Method::GET, "/v1/channels/channel/activity/stream");
    request
        .headers_mut()
        .insert("last-event-id", "x".repeat(1025).parse().unwrap());
    assert_eq!(f.send(request).await.status(), StatusCode::BAD_REQUEST);
    assert_eq!(f.control.calls.load(Ordering::SeqCst), 0);
}
