//! Shared helpers for the media retrieval tests.
use axum::Router;
use std::time::Duration;
use tokio::{net::TcpListener, time::Instant};

/// Serve `router` on a loopback port and return its origin.
pub async fn serve(router: Router) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    format!("http://{address}")
}

/// A deadline no well-behaved test reaches.
pub fn far() -> Instant {
    Instant::now() + Duration::from_secs(30)
}
