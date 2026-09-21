//! CMA Event Gateway HTTP service.

mod config;

use std::{future::IntoFuture, net::SocketAddr, time::Duration};

use axum::{routing::get, Json, Router};
use serde::Serialize;
use tokio::sync::oneshot;
use tower_http::limit::RequestBodyLimitLayer;
use utoipa::OpenApi;

pub use config::{Config, ConfigError, CMAEG_LISTEN};

const REQUEST_BODY_LIMIT_BYTES: usize = 1024 * 1024;
const HTTP_DRAIN_TIMEOUT: Duration = Duration::from_secs(15);
const DEFAULT_LOG_FILTER: &str = "cmaeg=info";

#[derive(Debug, Serialize)]
struct StatusResponse {
    status: &'static str,
}

/// The initial CMAEG OpenAPI document. Later issues add the versioned routes.
#[derive(Debug, OpenApi)]
#[openapi(info(title = "CMA Event Gateway", version = "0.1.0"))]
pub struct ApiDoc;

/// Build the stateless HTTP router.
pub fn router() -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/openapi.json", get(openapi))
        .layer(RequestBodyLimitLayer::new(REQUEST_BODY_LIMIT_BYTES))
}

async fn healthz() -> Json<StatusResponse> {
    Json(StatusResponse { status: "ok" })
}

async fn readyz() -> Json<StatusResponse> {
    Json(StatusResponse { status: "ready" })
}

async fn openapi() -> Json<utoipa::openapi::OpenApi> {
    Json(ApiDoc::openapi())
}

/// Run CMAEG until a process termination signal arrives.
pub async fn run() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let config = Config::from_env()?;
    let listener = tokio::net::TcpListener::bind(config.listen).await?;
    tracing::info!(bind = %config.listen, "CMAEG listening");

    let (shutdown_started, shutdown_signal) = oneshot::channel();
    let server = axum::serve(
        listener,
        router().into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async move {
        wait_for_shutdown_signal().await;
        let _ = shutdown_started.send(());
    })
    .into_future();
    tokio::pin!(server);

    tokio::select! {
        result = &mut server => result?,
        _ = shutdown_signal => {
            tokio::time::timeout(HTTP_DRAIN_TIMEOUT, &mut server)
                .await
                .map_err(|_| "CMAEG HTTP drain exceeded 15 seconds")??;
        }
    }
    Ok(())
}

async fn wait_for_shutdown_signal() {
    #[cfg(unix)]
    {
        let terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate());
        match terminate {
            Ok(mut signal) => {
                tokio::select! {
                    result = tokio::signal::ctrl_c() => {
                        if let Err(error) = result { tracing::warn!(%error, "ctrl-c signal listener stopped"); }
                    }
                    _ = signal.recv() => {}
                }
            }
            Err(error) => {
                tracing::warn!(%error, "SIGTERM listener unavailable; waiting for ctrl-c");
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

/// Initialize the container logger with the service's stable default filter.
pub fn init_logging() {
    tracing_subscriber::fmt()
        .with_ansi(false)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("RUST_LOG")
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(DEFAULT_LOG_FILTER)),
        )
        .init();
}
