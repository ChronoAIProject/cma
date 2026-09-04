//! Temporary runtime process while the supervisor contract is rebuilt by #807.

use std::net::SocketAddr;

use axum::{routing::get, Router};

const DEFAULT_LISTEN_ADDR: &str = "0.0.0.0:3001";

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let address = std::env::var("CHRONO_RUNTIME_LISTEN_ADDR")
        .unwrap_or_else(|_| DEFAULT_LISTEN_ADDR.into())
        .parse::<SocketAddr>()
        .unwrap_or_else(|error| panic!("invalid CHRONO_RUNTIME_LISTEN_ADDR: {error}"));
    let listener = tokio::net::TcpListener::bind(address)
        .await
        .unwrap_or_else(|error| panic!("could not bind runtime health listener: {error}"));
    let app = Router::new()
        .route("/livez", get(|| async { "ok" }))
        .route("/ready", get(|| async { "rebuilding" }));

    tracing::info!(%address, "runtime skeleton listening");
    axum::serve(listener, app)
        .await
        .unwrap_or_else(|error| panic!("runtime health server failed: {error}"));
}
