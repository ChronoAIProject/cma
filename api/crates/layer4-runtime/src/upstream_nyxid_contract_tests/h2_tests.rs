use super::{h2_peer::*, *};

#[tokio::test]
async fn default_policy_retries_refused_h2_stream_with_frozen_body_and_credential() {
    let mut peer = H2Peer::start().await;
    let credential = credential().await;
    let task = send(peer.router(credential.clone(), reqwest::Client::builder(), false));
    let mut first = peer.next().await;
    assert_request(&first.request);
    credential
        .install(&CredentialFrame {
            operation_id: "h2-contract-rotate".into(),
            value: RuntimeCredential::new("h2-rotated-fixture"),
            custody_generation: Some(2),
            github_route: None,
        })
        .await
        .unwrap();
    first.reply.send_reset(h2::Reason::REFUSED_STREAM);
    let mut second = peer.next().await;
    assert_request(&second.request);
    assert_eq!(first.request.uri(), second.request.uri());
    assert_eq!(first.request.headers(), second.request.headers());
    let mut response = second
        .reply
        .send_response(
            axum::http::Response::builder()
                .status(200)
                .body(())
                .unwrap(),
            false,
        )
        .unwrap();
    response
        .send_data(Bytes::from_static(b"accepted"), true)
        .unwrap();
    let response = finish(task).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        to_bytes(response.into_body(), 4096).await.unwrap(),
        "accepted"
    );
    assert_eq!(peer.stream_count(), 2);
    peer.stop().await;
}

#[tokio::test]
async fn explicit_no_retry_control_returns_502_after_one_refused_h2_stream() {
    let mut peer = H2Peer::start().await;
    let task = send(peer.router(
        credential().await,
        reqwest::Client::builder().retry(reqwest::retry::never()),
        false,
    ));
    let mut exchange = peer.next().await;
    assert_request(&exchange.request);
    exchange.reply.send_reset(h2::Reason::REFUSED_STREAM);
    assert_failed(task).await;
    assert_eq!(peer.stream_count(), 1);
    peer.stop().await;
}

#[tokio::test]
async fn default_policy_bounds_repeated_h2_refusals_to_three_attempts() {
    let mut peer = H2Peer::start().await;
    let task = send(peer.router(credential().await, reqwest::Client::builder(), false));
    for _ in 0..3 {
        let mut exchange = peer.next().await;
        assert_request(&exchange.request);
        exchange.reply.send_reset(h2::Reason::REFUSED_STREAM);
    }
    assert_failed(task).await;
    assert_eq!(peer.stream_count(), 3);
    peer.stop().await;
}

#[tokio::test]
async fn default_policy_does_not_retry_non_refusal_h2_resets() {
    for reason in [h2::Reason::CANCEL, h2::Reason::INTERNAL_ERROR] {
        let mut peer = H2Peer::start().await;
        let task = send(peer.router(credential().await, reqwest::Client::builder(), false));
        let mut exchange = peer.next().await;
        assert_request(&exchange.request);
        exchange.reply.send_reset(reason);
        assert_failed(task).await;
        assert_eq!(peer.stream_count(), 1);
        peer.stop().await;
    }
}

#[tokio::test]
async fn missing_credential_and_unsupported_funding_open_no_h2_connection() {
    let peer = H2Peer::start().await;
    for (credential, required, expected) in [
        (
            CredentialHolder::new(),
            false,
            "no upstream credential available yet",
        ),
        (credential().await, true, "model_funding_unsupported"),
        (CredentialHolder::new(), true, "model_funding_unsupported"),
    ] {
        let response = finish(send(peer.router(
            credential,
            reqwest::Client::builder(),
            required,
        )))
        .await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            to_bytes(response.into_body(), 4096).await.unwrap(),
            expected
        );
    }
    assert_eq!(peer.connection_count(), 0);
    assert_eq!(peer.stream_count(), 0);
    peer.stop().await;
}

async fn assert_failed(task: tokio::task::JoinHandle<Response>) {
    let response = finish(task).await;
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    assert_eq!(
        to_bytes(response.into_body(), 4096).await.unwrap(),
        "upstream request failed"
    );
}
