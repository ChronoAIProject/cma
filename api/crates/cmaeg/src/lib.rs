//! CMA Event Gateway HTTP service.

pub mod auth;
pub mod channels;
mod config;
pub mod http;
pub mod ingress;
pub mod providers;
pub mod records;

use std::{future::IntoFuture, net::SocketAddr, time::Duration};

use axum::{routing::get, Json, Router};
use serde::Serialize;
use tokio::sync::oneshot;
use tower_http::limit::RequestBodyLimitLayer;
use utoipa::OpenApi;

pub use config::{
    ChannelCacheConfig, Config, ConfigError, IngressConfig, ManagementConfig,
    CMAEG_CHANNEL_CACHE_MAX, CMAEG_CHANNEL_CACHE_TTL_MS, CMAEG_CHANNEL_NEGATIVE_TTL_MS,
    CMAEG_CMA_BASE_URL, CMAEG_CMA_SERVICE_TOKEN_FILE, CMAEG_INGRESS_INFLIGHT,
    CMAEG_INGRESS_MAX_BODY, CMAEG_JWKS_TTL_MS, CMAEG_LISTEN, CMAEG_MASTER_KEYS_FILE,
    CMAEG_NYXID_BASE_URL, CMAEG_PROVIDERS, CMAEG_PROVIDER_CONNECT_TIMEOUT_MS,
    CMAEG_PROVIDER_MAX_INFLIGHT, CMAEG_PROVIDER_REQUEST_TIMEOUT_MS, CMAEG_PUBLIC_URL,
    CMAEG_RESTORE_RATE_PER_MINUTE, CMAEG_RESTORE_TIMEOUT_MS, CMAEG_TOOLS_NYXID_SLUG,
};

const REQUEST_BODY_LIMIT_BYTES: usize = 1024 * 1024;
const HTTP_DRAIN_TIMEOUT: Duration = Duration::from_secs(15);
const DEFAULT_LOG_FILTER: &str = "cmaeg=info";

#[derive(Debug, Serialize)]
struct StatusResponse {
    status: &'static str,
}

/// The versioned CMAEG management API document.
#[derive(Debug, OpenApi)]
#[openapi(
    info(title = "CMA Event Gateway", version = "0.1.0"),
    modifiers(&ManagementSecurity),
    paths(
        http::management::create_channel,
        http::management::get_channel,
        http::management::update_channel,
        http::management::delete_channel,
        http::management::test_channel,
        http::management::rekey_channel,
        http::callbacks::callback,
    )
)]
pub struct ApiDoc;

struct ManagementSecurity;
impl utoipa::Modify for ManagementSecurity {
    fn modify(&self, document: &mut utoipa::openapi::OpenApi) {
        use utoipa::openapi::security::{HttpAuthScheme, HttpBuilder, SecurityScheme};
        let components = document.components.get_or_insert_with(Default::default);
        for (name, description) in [
            ("creatorBearer", "NyxID creator bearer token"),
            ("serviceBearer", "CMA service token; rekey only"),
        ] {
            components.add_security_scheme(
                name,
                SecurityScheme::Http(
                    HttpBuilder::new()
                        .scheme(HttpAuthScheme::Bearer)
                        .description(Some(description))
                        .build(),
                ),
            );
        }
    }
}

/// Build the stateless HTTP router.
pub fn router() -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route(
            "/readyz",
            get(http::readiness::readyz).with_state(http::readiness::Readiness::default()),
        )
        .route("/openapi.json", get(openapi))
        .layer(RequestBodyLimitLayer::new(REQUEST_BODY_LIMIT_BYTES))
        .layer(axum::middleware::from_fn(http::problems::protocol_headers))
}

/// Build the complete stateful router used by the running service.
pub fn router_with_state(state: http::ManagementState) -> Router {
    let readiness = state.readiness.clone();
    http::management::management_router(state)
        .route("/healthz", get(healthz))
        .route(
            "/readyz",
            get(http::readiness::readyz).with_state(readiness),
        )
        .route("/openapi.json", get(openapi))
        .layer(RequestBodyLimitLayer::new(REQUEST_BODY_LIMIT_BYTES))
        .layer(axum::middleware::from_fn(http::problems::protocol_headers))
}

async fn healthz() -> Json<StatusResponse> {
    Json(StatusResponse { status: "ok" })
}

async fn openapi() -> Json<utoipa::openapi::OpenApi> {
    Json(ApiDoc::openapi())
}

/// Run CMAEG until a process termination signal arrives.
pub async fn run() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let config = Config::from_env()?;
    let providers = std::sync::Arc::new(providers::ProviderClient::from_config(&config)?);
    let records = std::sync::Arc::new(records::RecordClient::new(
        &config.management.cma_base_url,
        config.management.cma_service_token.as_str(),
    )?);
    let creator_auth = std::sync::Arc::new(auth::CreatorAuthenticator::new(
        &config.management.nyxid_base_url,
    )?);
    let state = http::ManagementState::new(
        providers,
        records,
        creator_auth,
        config.management.clone(),
        config.channel_cache,
    );
    let _reachability_probe = state.readiness.start_probe(state.records.clone());
    let ingress = ingress::IngressState::new(
        state.resolver.clone(),
        state.master_keys.clone(),
        config.ingress,
        std::sync::Arc::new(ingress::launch::UnavailableLauncher),
    )?;
    let listener = tokio::net::TcpListener::bind(config.listen).await?;
    tracing::info!(bind = %config.listen, "CMAEG listening");

    let (shutdown_started, shutdown_signal) = oneshot::channel();
    let server = axum::serve(
        listener,
        router_with_state(state)
            .merge(http::callbacks::router(ingress))
            .into_make_service_with_connect_info::<SocketAddr>(),
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
