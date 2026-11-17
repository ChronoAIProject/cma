use super::{seed_path, Fixture};
use serde_json::Value;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

async fn cancelled_connection_then_read(request_prefix: &[u8]) {
    let fixture = Fixture::new().await;
    let before = fixture.state.lock().unwrap().rows.clone();
    let mut socket = tokio::net::TcpStream::connect(fixture.address)
        .await
        .unwrap();
    socket.write_all(request_prefix).await.unwrap();
    socket.shutdown().await.unwrap();
    // Wait for the server to finish this connection before issuing the retry.
    // This proves accept-loop survival without scheduling sleeps or retry loops.
    let mut response = Vec::new();
    tokio::time::timeout(Duration::from_secs(2), socket.read_to_end(&mut response))
        .await
        .expect("fixture did not finish the cancelled connection")
        .unwrap();
    assert!(response.is_empty(), "an incomplete request got a response");

    let request = http::Request::get(seed_path()).body(Vec::new()).unwrap();
    let observed = tokio::time::timeout(
        Duration::from_secs(2),
        fixture.kubernetes.request::<Value>(request),
    )
    .await
    .expect("fixture stopped accepting after cancelled connection")
    .expect("fixture rejected request after cancelled connection");
    assert_eq!(observed, before[seed_path()]);
    let state = fixture.state.lock().unwrap();
    assert_eq!(state.rows, before, "cancelled request changed resources");
    assert_eq!(
        state.calls,
        [(format!("GET {}", seed_path()), Value::Null)],
        "only the complete request may reach the resource handler"
    );
}

#[tokio::test]
async fn cancelled_empty_connection_keeps_fixture_available() {
    cancelled_connection_then_read(b"").await;
}

#[tokio::test]
async fn cancelled_request_body_keeps_fixture_available() {
    let request = format!(
        "PATCH {} HTTP/1.1\r\nHost: fixture\r\nContent-Length: 20\r\n\r\n{{",
        seed_path()
    );
    cancelled_connection_then_read(request.as_bytes()).await;
}
