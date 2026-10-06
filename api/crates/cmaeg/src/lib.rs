//! CMA Event Gateway HTTP service.

pub mod auth;
pub mod channels;
mod config;
pub mod connections;
pub mod delivery;
pub mod http;
pub mod ingress;
pub mod interactions;
pub mod mcp;
pub mod openapi;
pub mod providers;
pub mod records;
mod service_auth;
pub mod telemetry;
pub mod tools;
pub mod turns;

use std::{future::IntoFuture, net::SocketAddr, time::Instant};

use axum::{routing::get, Json, Router};
use serde::Serialize;
use tokio::sync::oneshot;
use tower_http::limit::RequestBodyLimitLayer;
use utoipa::OpenApi;

pub use config::{
    ChannelCacheConfig, Config, ConfigError, DeliveryConfig, IngressConfig, ManagementConfig,
    ServiceAccountConfig, TurnConfig, CMAEG_ACCEPT_BUDGET_MS, CMAEG_ACK_TEXT,
    CMAEG_BUSY_RETRY_MAX_MS, CMAEG_CHANNEL_CACHE_MAX, CMAEG_CHANNEL_CACHE_TTL_MS,
    CMAEG_CHANNEL_NEGATIVE_TTL_MS, CMAEG_CMA_BASE_URL, CMAEG_CMA_SERVICE_TOKEN_FILE,
    CMAEG_DRAIN_SECONDS, CMAEG_FAILURE_TEXT, CMAEG_HEARTBEAT_MS, CMAEG_INGRESS_INFLIGHT,
    CMAEG_INGRESS_MAX_BODY, CMAEG_JWKS_TTL_MS, CMAEG_LISTEN, CMAEG_MASTER_KEYS_FILE,
    CMAEG_METRICS_LISTEN, CMAEG_NYXID_BASE_URL, CMAEG_NYXID_REPLY_TIMEOUT_MS, CMAEG_PROVIDERS,
    CMAEG_PROVIDER_CONNECT_TIMEOUT_MS, CMAEG_PROVIDER_MAX_INFLIGHT,
    CMAEG_PROVIDER_REQUEST_TIMEOUT_MS, CMAEG_PUBLIC_URL, CMAEG_RESTORE_RATE_PER_MINUTE,
    CMAEG_RESTORE_TIMEOUT_MS, CMAEG_TOOLS_NYXID_SLUG, CMAEG_TURN_TIMEOUT_MS,
};

const REQUEST_BODY_LIMIT_BYTES: usize = 1024 * 1024;
const DEFAULT_LOG_FILTER: &str = "cmaeg=info";

#[derive(Debug, Serialize)]
struct StatusResponse {
    status: &'static str,
}

pub use openapi::ApiDoc;

/// Build the stateless HTTP router.
pub fn router() -> Router {
    telemetry::http::instrument(
        Router::new()
            .route("/healthz", get(healthz))
            .route(
                "/readyz",
                get(http::readiness::readyz).with_state(http::readiness::Readiness::default()),
            )
            .route("/openapi.json", get(openapi))
            .merge(http::docs::router())
            .layer(RequestBodyLimitLayer::new(REQUEST_BODY_LIMIT_BYTES))
            .layer(axum::middleware::from_fn(http::problems::protocol_headers)),
    )
}

/// Build the complete stateful router used by the running service.
pub fn router_with_state(state: http::ManagementState) -> Router {
    let readiness = state.readiness.clone();
    telemetry::http::instrument(
        http::management::management_router(state)
            .route("/healthz", get(healthz))
            .route(
                "/readyz",
                get(http::readiness::readyz).with_state(readiness),
            )
            .route("/openapi.json", get(openapi))
            .merge(http::docs::router())
            .layer(RequestBodyLimitLayer::new(REQUEST_BODY_LIMIT_BYTES))
            .layer(axum::middleware::from_fn(http::problems::protocol_headers)),
    )
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
    let service_auth = service_auth::ServiceAuth::from_config(&config)?;
    // Prove the renewable credential before advertising gateway readiness.
    service_auth.bearer().await?;
    let providers = std::sync::Arc::new(
        providers::ProviderClient::from_config(&config)?.with_service_auth(service_auth.clone()),
    );
    let records = std::sync::Arc::new(
        records::RecordClient::new(
            &config.management.cma_base_url,
            config.management.cma_service_token.as_str(),
        )?
        .with_service_auth(service_auth),
    );
    let creator_auth = std::sync::Arc::new(auth::CreatorAuthenticator::new(
        &config.management.nyxid_base_url,
    )?);
    let state = http::ManagementState::new(
        providers.clone(),
        records,
        creator_auth,
        config.management.clone(),
        config.channel_cache,
    );
    let _reachability_probe = state.readiness.start_probe(state.records.clone());
    let tracker = turns::TurnTracker::new(state.readiness.clone());
    let (delivery, outcomes) = tokio::sync::mpsc::channel(256);
    let waiters = std::sync::Arc::new(delivery::sync::SyncRegistry::default());
    let interactions = std::sync::Arc::new(interactions::InteractionStore::new(
        (*state.records).clone(),
        state.master_keys.clone(),
        providers.clone(),
    ));
    let delivery_engine = delivery::engine::DeliveryEngine::start(
        outcomes,
        state.master_keys.clone(),
        delivery::relay_client::RelayClient::new(
            &config.management.nyxid_base_url,
            config.delivery.reply_timeout,
        )?
        .with_interactions(interactions.clone()),
        config.delivery.clone(),
        tracker.clone(),
        delivery::custom::CustomDelivery::new(
            config.management.public_url.clone(),
            waiters.clone(),
        ),
    );
    let runner = std::sync::Arc::new(turns::TurnRunner::new(
        providers.clone(),
        tracker.clone(),
        delivery.clone(),
        config.turns.clone(),
    ));
    let ingress = ingress::IngressState::new(
        state.resolver.clone(),
        state.master_keys.clone(),
        config.ingress,
        runner,
    )?
    .with_interactions(interactions);
    let tools_state = tools::ToolsState {
        resolver: state.resolver.clone(),
        keys: state.master_keys.clone(),
        providers,
        delivery,
        tracker: tracker.clone(),
        public_origin: config.management.public_url.clone(),
    };
    let listener = tokio::net::TcpListener::bind(config.listen).await?;
    let _metrics =
        http::metrics::MetricsServer::start(config.metrics_listen, telemetry::install()?).await?;
    tracing::info!(bind = %config.listen, "CMAEG listening");

    let (shutdown_started, shutdown_signal) = oneshot::channel();
    let server = axum::serve(
        listener,
        router_with_state(state)
            .merge(http::events::router(http::events::CustomIngressState {
                ingress: ingress.clone(),
                waiters,
            }))
            .merge(http::callbacks::router(ingress))
            .merge(http::tools::router(tools_state.clone()))
            .merge(mcp::router(tools_state))
            .into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async move {
        wait_for_shutdown_signal().await;
        tracker.start_draining();
        let deadline = Instant::now() + config.turns.drain;
        if !tracker.drain(deadline).await {
            tracing::warn!("turn drain deadline reached; remaining consumers cancelled");
        }
        if !delivery_engine.drain(deadline).await {
            tracing::warn!(
                "delivery drain deadline reached; remaining sends cancelled with unknown outcome"
            );
        }
        let _ = shutdown_started.send(deadline);
    })
    .into_future();
    tokio::pin!(server);

    tokio::select! {
        result = &mut server => result?,
        deadline = shutdown_signal => {
            let deadline = deadline?;
            match tokio::time::timeout_at(deadline.into(), &mut server).await {
                Ok(result) => result?,
                Err(_) => tracing::warn!("HTTP drain deadline reached"),
            }
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
