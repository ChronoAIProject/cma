use super::*;
use axum::{body::to_bytes, routing::get, Router};
use std::{sync::Arc, time::Duration};
use tokio::sync::Notify;

fn public_response() -> Response {
    let mut response = Response::new(Body::from("original response body"));
    *response.status_mut() = StatusCode::UNPROCESSABLE_ENTITY;
    response.headers_mut().insert(
        "content-type",
        HeaderValue::from_static("application/problem+json"),
    );
    response
        .headers_mut()
        .insert("retry-after", HeaderValue::from_static("1"));
    response
        .headers_mut()
        .append("set-cookie", HeaderValue::from_static("first=one"));
    response
        .headers_mut()
        .append("set-cookie", HeaderValue::from_static("second=two"));
    response
}

async fn assert_public(response: Response) {
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(response.headers(), public_response().headers());
    assert_eq!(
        to_bytes(response.into_body(), 1024).await.unwrap(),
        "original response body"
    );
}

#[tokio::test]
async fn admission_headers_arrive_before_handler_and_preserve_its_refusal() {
    let ready = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let app = Router::new().route(
        "/",
        get({
            let ready = ready.clone();
            let release = release.clone();
            move || {
                let ready = ready.clone();
                let release = release.clone();
                async move {
                    acknowledge(async move {
                        ready.notify_one();
                        release.notified().await;
                        public_response()
                    })
                }
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let admitted = tokio::time::timeout(Duration::from_secs(1), client.get(url).send())
        .await
        .unwrap()
        .unwrap();
    assert!(requested(admitted.headers()));
    ready.notified().await;
    let decoded = response(admitted);
    tokio::pin!(decoded);
    assert!(futures_util::poll!(&mut decoded).is_pending());
    release.notify_one();
    assert_public(decoded.await.unwrap()).await;
    server.abort();
}

#[tokio::test]
async fn envelope_handles_split_and_coalesced_head_without_changing_public_bytes() {
    let encoded = to_bytes(
        acknowledge(async { public_response() }).into_body(),
        MAX_HEAD,
    )
    .await
    .unwrap();
    for size in [1, 3, 4, 17, MAX_HEAD] {
        let chunks: Vec<_> = encoded
            .chunks(size)
            .map(|bytes| Ok::<_, io::Error>(Bytes::copy_from_slice(bytes)))
            .collect();
        assert_public(decode(stream::iter(chunks)).await.unwrap()).await;
    }
}

#[tokio::test]
async fn dropping_unfinished_reconstruction_cancels_it_without_background_work() {
    let (sender, receiver) = tokio::sync::oneshot::channel::<()>();
    let response = acknowledge(async move {
        let _sender = sender;
        std::future::pending::<Response>().await
    });
    let mut body = response.into_body().into_data_stream();
    assert!(futures_util::poll!(body.next()).is_pending());
    drop(body);
    assert!(receiver.await.is_err());
}

#[tokio::test]
async fn corrupt_or_unbounded_envelope_never_becomes_a_successful_public_response() {
    let valid = encode(&public_response()).unwrap();
    let mut invalid = vec![
        vec![],
        vec![0],
        vec![0, 0, 0, 0],
        ((MAX_HEAD + 1) as u32).to_be_bytes().to_vec(),
    ];
    for end in [3, 4, valid.len() - 1] {
        invalid.push(valid[..end].to_vec());
    }
    for head in [
        br#"not json"#.as_slice(),
        br#"{"status":100,"headers":[]}"#,
        br#"{"status":200,"headers":[["bad\nname",[]]]}"#,
        br#"{"status":200,"headers":[["name",[13,10]]]}"#,
    ] {
        let mut bytes = (head.len() as u32).to_be_bytes().to_vec();
        bytes.extend_from_slice(head);
        invalid.push(bytes);
    }
    for bytes in invalid {
        assert!(
            decode(stream::iter([Ok::<_, io::Error>(Bytes::from(bytes))]))
                .await
                .is_err()
        );
    }
    let stream = stream::iter([Ok(valid), Err(io::Error::other("read failed"))]);
    let response = decode(stream).await.unwrap();
    assert!(to_bytes(response.into_body(), 1024).await.is_err());
    let mut oversized = public_response();
    oversized.headers_mut().insert(
        "large",
        HeaderValue::from_bytes(&vec![b'a'; MAX_HEAD]).unwrap(),
    );
    assert!(encode(&oversized).is_err());
}

#[tokio::test]
async fn old_peer_public_response_remains_compatible() {
    let app = Router::new().route("/", get(|| async { public_response() }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let peer = reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .get(url)
        .send()
        .await
        .unwrap();
    let headers = peer.headers().clone();
    let response = response(peer).await.unwrap();
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(response.headers(), &headers);
    assert_eq!(
        to_bytes(response.into_body(), 1024).await.unwrap(),
        "original response body"
    );
    server.abort();
}

#[test]
fn negotiation_requires_one_supported_version() {
    let mut headers = HeaderMap::new();
    assert!(!requested(&headers));
    headers.insert(HEADER, HeaderValue::from_static("unknown"));
    assert!(!requested(&headers));
    headers.insert(HEADER, HeaderValue::from_static(VERSION));
    assert!(requested(&headers));
    headers.append(HEADER, HeaderValue::from_static(VERSION));
    assert!(!requested(&headers));
}
