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
    // Give the cancelled connection a bounded wait for EOF. Issue the next GET
    // even if that wait expires, so the test also checks accept-loop survival.
    let mut response = Vec::new();
    let cancellation =
        tokio::time::timeout(Duration::from_secs(2), socket.read_to_end(&mut response)).await;
    let closed = matches!(&cancellation, Ok(Ok(_)));

    let request = http::Request::get(seed_path()).body(Vec::new()).unwrap();
    let observed = tokio::time::timeout(
        Duration::from_secs(2),
        fixture.kubernetes.request::<Value>(request),
    )
    .await
    .unwrap_or_else(|_| {
        panic!(
            "fixture stopped serving after cancellation (cancelled socket reached EOF: {closed})"
        )
    })
    .expect("fixture rejected request after cancelled connection");
    cancellation
        .expect("fixture did not finish the cancelled connection")
        .unwrap();
    assert!(response.is_empty(), "an incomplete request got a response");
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
async fn cancelled_partial_header_keeps_fixture_available() {
    let request = format!("PATCH {} HTTP/1.1\r\nHost: fixt", seed_path());
    cancelled_connection_then_read(request.as_bytes()).await;
}

#[tokio::test]
async fn cancelled_request_body_keeps_fixture_available() {
    let request = format!(
        "PATCH {} HTTP/1.1\r\nHost: fixture\r\nContent-Length: 20\r\n\r\n{{",
        seed_path()
    );
    cancelled_connection_then_read(request.as_bytes()).await;
}

#[tokio::test]
async fn malformed_complete_request_still_fails_the_fixture() {
    let mut fixture = Fixture::new().await;
    let before = fixture.state.lock().unwrap().rows.clone();
    let mut socket = tokio::net::TcpStream::connect(fixture.address)
        .await
        .unwrap();
    let request = format!(
        "PATCH {} HTTP/1.1\r\nHost: fixture\r\nContent-Length: 1\r\n\r\n{{",
        seed_path()
    );
    socket.write_all(request.as_bytes()).await.unwrap();
    socket.shutdown().await.unwrap();
    let error = tokio::time::timeout(Duration::from_secs(2), &mut fixture.task)
        .await
        .expect("fixture did not handle a complete malformed request")
        .expect_err("fixture silently accepted invalid JSON");
    assert!(error.is_panic());
    let state = fixture.state.lock().unwrap();
    assert_eq!(state.rows, before);
    assert!(state.calls.is_empty());
}
