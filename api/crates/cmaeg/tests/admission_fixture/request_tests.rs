use super::{protocol::Signed, request::post};
use axum::{
    body::{to_bytes, Body},
    extract::Request,
    http::StatusCode,
    response::{IntoResponse, Response},
    Router,
};
use std::time::{Duration, Instant};

fn signed() -> Signed {
    Signed {
        body: br#"{"fixture":"synthetic"}"#.to_vec(),
        callback: uuid::Uuid::new_v4().to_string(),
        reply: uuid::Uuid::new_v4().to_string(),
        timestamp: "2026-11-14T00:00:00Z".into(),
    }
}
async fn serve(
    router: Router,
) -> (
    String,
    tokio::task::JoinHandle<()>,
    tokio::sync::oneshot::Sender<()>,
) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(async {
                let _ = stopped.await;
            })
            .await
            .unwrap();
    });
    (base, task, stop)
}
async fn finish(task: tokio::task::JoinHandle<()>, stop: tokio::sync::oneshot::Sender<()>) {
    stop.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(1), task)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn requests_preserve_signed_bytes_identity_and_observed_response_code_without_raw_body() {
    let original = signed();
    let expected = original.clone();
    let router = Router::new().fallback(move |request: Request| {
        let expected = expected.clone();
        async move {
            assert_eq!(
                request.headers()["x-nyxid-callback-token"],
                expected.callback
            );
            assert_eq!(request.headers()["x-nyxid-message-id"], "event");
            assert_eq!(request.headers()["x-nyxid-timestamp"], expected.timestamp);
            let id = request.headers()["x-request-id"].clone();
            assert_eq!(
                to_bytes(request.into_body(), 16384).await.unwrap(),
                expected.body
            );
            Response::builder()
                .status(202)
                .header("x-request-id", id)
                .body(Body::from(r#"{"status":"accepted"}"#))
                .unwrap()
        }
    });
    let (base, task, stop) = serve(router).await;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(1))
        .build()
        .unwrap();
    let row = post(
        &client,
        &base,
        "channel",
        "event",
        &original,
        1,
        Instant::now(),
    )
    .await;
    assert_eq!(row["status"], 202);
    assert_eq!(row["accepted_body"], true);
    assert_eq!(row["request_id_echo_matches"], true);
    assert!(!row.to_string().contains(&original.callback));
    assert!(!row.to_string().contains("synthetic"));
    finish(task, stop).await;
}

#[tokio::test]
async fn refusals_and_oversized_bodies_remain_failed_attempts() {
    for oversized in [false, true] {
        let router = Router::new().fallback(move || async move {
            if oversized {
                (StatusCode::SERVICE_UNAVAILABLE, "X".repeat(20000)).into_response()
            } else {
                (
                    StatusCode::SERVICE_UNAVAILABLE,
                    r#"{"code":"provider_unavailable","title":"provider unavailable","status":503,"detail":"PRIVATE_CANARY"}"#,
                )
                    .into_response()
            }
        });
        let (base, task, stop) = serve(router).await;
        let row = post(
            &reqwest::Client::new(),
            &base,
            "channel",
            "event",
            &signed(),
            1,
            Instant::now(),
        )
        .await;
        assert_eq!(row["status"], 503);
        assert_eq!(row["accepted_body"], false);
        assert!(!row.to_string().contains("PRIVATE_CANARY"));
        if oversized {
            assert_eq!(row["body_complete"], false);
        } else {
            assert_eq!(row["public_code"], "provider_unavailable");
        }
        finish(task, stop).await;
    }
}

#[tokio::test]
async fn whole_request_deadline_retains_timeout_instead_of_a_success_status() {
    let router = Router::new().fallback(|| async {
        tokio::time::sleep(Duration::from_millis(100)).await;
        StatusCode::OK
    });
    let (base, task, stop) = serve(router).await;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_millis(10))
        .build()
        .unwrap();
    let row = post(
        &client,
        &base,
        "channel",
        "event",
        &signed(),
        1,
        Instant::now(),
    )
    .await;
    assert_eq!(row["transport_outcome"], "timeout");
    assert!(row.get("status").is_none());
    finish(task, stop).await;
}
