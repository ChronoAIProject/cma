use axum::{extract::Extension, http::StatusCode, routing::post, Router};
use cmaeg::telemetry::{self, timing::IngressTiming};
use std::{
    io::{self, Write},
    sync::{Arc, Mutex},
    time::Instant,
};

#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<u8>>>);
impl Write for Capture {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn first_write_after_response_keeps_original_request_correlation() {
    let capture = Capture::default();
    let writer = capture.clone();
    tracing::subscriber::set_global_default(
        tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_writer(move || writer.clone())
            .finish(),
    )
    .unwrap();
    let (send, mut receive) = tokio::sync::mpsc::channel(1);
    let app = telemetry::http::instrument(Router::new().route(
        "/v1/channels/{channel_id}/events",
        post(move |Extension(timing): Extension<IngressTiming>| {
            let send = send.clone();
            async move {
                timing.received();
                timing.triggered_restore();
                send.send(timing).await.unwrap();
                StatusCode::ACCEPTED
            }
        }),
    ));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let response = reqwest::Client::new()
        .post(format!("http://{address}/v1/channels/test/events"))
        .header("x-request-id", "deferred-request")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let header = response.headers()["server-timing"].to_str().unwrap();
    assert!(header.contains("not_written"));
    let timing = receive.recv().await.unwrap();
    let before = String::from_utf8(capture.0.lock().unwrap().clone()).unwrap();
    assert!(before.lines().any(|line| line.contains("deferred-request")
        && line.contains("request finished")
        && line.contains("not_written")));
    assert!(!before.contains("ingress socket write"));

    // Simulate the transport notification after the HTTP response and span exit.
    timing.written(Instant::now());
    timing.written(Instant::now());
    assert!(!timing.header().contains("not_written"));
    let after = String::from_utf8(capture.0.lock().unwrap().clone()).unwrap();
    let writes: Vec<_> = after
        .lines()
        .filter(|line| line.contains("ingress socket write"))
        .collect();
    assert_eq!(writes.len(), 1);
    assert!(writes[0].contains("deferred-request"));
    assert!(writes[0].contains("/v1/channels/{channel_id}/events"));
    assert!(writes[0].contains("cold=true"));
    assert!(writes[0].contains("ingress_to_provider_ms="));
    server.abort();
    assert!(server.await.unwrap_err().is_cancelled());
}
