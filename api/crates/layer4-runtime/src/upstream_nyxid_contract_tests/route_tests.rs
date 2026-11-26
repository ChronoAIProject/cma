use super::*;

fn request(uri: &str, body: Body) -> Request {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::AUTHORIZATION, "Bearer caller-fixture")
        .header("open-sandbox-api-key", "caller-provider-fixture")
        .header(header::CONTENT_TYPE, "application/json")
        .body(body)
        .unwrap()
}

async fn reply_ok(reply: oneshot::Sender<Response>, task: tokio::task::JoinHandle<Response>) {
    reply.send(Response::new(Body::from("accepted"))).unwrap();
    let response = finish(task).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        to_bytes(response.into_body(), 4096).await.unwrap(),
        "accepted"
    );
}

#[tokio::test]
async fn ordinary_and_continuation_requests_keep_configured_origin_and_exact_payload() {
    let mut peer = Peer::start().await;
    let alternate = Peer::start().await;
    for body in [
        r#"{ "model": "fixture", "input": "ordinary" }"#,
        r#"{ "previous_response_id": "resp_fixture", "input": [{"type":"function_call_output","call_id":"call_fixture","output":"continued"}], "stream":true }"#,
    ] {
        let path = "/v1/responses?trace=one%2Ftwo&mode=continuation";
        let mut outbound = request(&format!("{}{path}", alternate.origin), Body::from(body));
        outbound.headers_mut().insert(
            header::HOST,
            alternate
                .origin
                .trim_start_matches("http://")
                .parse()
                .unwrap(),
        );
        let task = peer.send_request(outbound);
        let Exchange { request, reply } = peer.next().await;
        assert_eq!(request.method(), "POST");
        assert_eq!(
            request.uri().to_string(),
            format!("/api/v1/proxy/s/chrono-llm-public{path}")
        );
        assert_eq!(
            request.headers()[header::HOST],
            peer.origin.trim_start_matches("http://")
        );
        assert_eq!(
            request.headers()[header::AUTHORIZATION],
            "Bearer contract-scoped-fixture"
        );
        assert_eq!(request.headers()[header::ACCEPT_ENCODING], "identity");
        assert!(!request.headers().contains_key("open-sandbox-api-key"));
        assert_eq!(to_bytes(request.into_body(), 4096).await.unwrap(), body);
        reply_ok(reply, task).await;
    }
    assert_eq!(peer.count.load(Ordering::SeqCst), 2);
    assert_eq!(alternate.count.load(Ordering::SeqCst), 0);
    peer.stop().await;
    alternate.stop().await;
}

#[tokio::test]
async fn cross_origin_307_and_308_are_returned_without_following_them() {
    let mut peer = Peer::start().await;
    let alternate = Peer::start().await;
    for status in [307, 308] {
        let task = peer.send();
        let exchange = peer.next().await;
        let destination = format!("{}/redirected", alternate.origin);
        exchange
            .reply
            .send(
                Response::builder()
                    .status(status)
                    .header(header::LOCATION, &destination)
                    .body(Body::from("redirect body"))
                    .unwrap(),
            )
            .unwrap();
        let response = finish(task).await;
        assert_eq!(response.status(), status);
        assert_eq!(response.headers()[header::LOCATION], destination);
        assert_eq!(
            to_bytes(response.into_body(), 4096).await.unwrap(),
            "redirect body"
        );
    }
    assert_eq!(peer.count.load(Ordering::SeqCst), 2);
    assert_eq!(alternate.count.load(Ordering::SeqCst), 0);
    peer.stop().await;
    alternate.stop().await;
}

#[tokio::test]
async fn unsafe_paths_are_refused_before_any_upstream_request() {
    let peer = Peer::start().await;
    for path in ["/../secret", "/%2e%2e/secret", "/%252e%252e/secret"] {
        let response = finish(peer.send_request(request(path, Body::empty()))).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{path}");
    }
    assert_eq!(peer.count.load(Ordering::SeqCst), 0);
    peer.stop().await;
}

#[tokio::test]
async fn credential_rotation_applies_after_the_in_flight_request_snapshot() {
    let mut peer = Peer::start().await;
    let (polled, ready) = oneshot::channel();
    let (release, released) = oneshot::channel();
    let body = Body::from_stream(futures_util::stream::once(async move {
        polled.send(()).unwrap();
        released.await.unwrap();
        Ok::<_, std::io::Error>(Bytes::from_static(b"{\"input\":\"snapshot\"}"))
    }));
    let task = peer.send_request(request("/v1/responses", body));
    // The handler reads the installed credential before it polls the caller body.
    tokio::time::timeout(Duration::from_secs(2), ready)
        .await
        .unwrap()
        .unwrap();
    peer.credential
        .install(&CredentialFrame {
            operation_id: "contract-rotate".into(),
            value: RuntimeCredential::new("contract-rotated-fixture"),
            custody_generation: Some(2),
            github_route: None,
        })
        .await
        .unwrap();
    release.send(()).unwrap();
    let exchange = peer.next().await;
    assert_eq!(
        exchange.request.headers()[header::AUTHORIZATION],
        "Bearer contract-scoped-fixture"
    );
    // Response guarding uses the same old snapshot even after a new key is installed.
    exchange
        .reply
        .send(Response::new(Body::from("contract-scoped-fixture")))
        .unwrap();
    let response = finish(task).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(to_bytes(response.into_body(), 4096).await.is_err());

    let task = peer.send();
    let exchange = peer.next().await;
    assert_eq!(
        exchange.request.headers()[header::AUTHORIZATION],
        "Bearer contract-rotated-fixture"
    );
    reply_ok(exchange.reply, task).await;
    assert_eq!(peer.count.load(Ordering::SeqCst), 2);
    peer.stop().await;
}
