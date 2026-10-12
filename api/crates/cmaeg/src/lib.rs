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
pub mod runtime;
mod service_auth;
pub mod telegram;
pub mod telemetry;
pub mod tools;
pub mod turns;

use std::{future::IntoFuture, net::SocketAddr, time::Instant};

use axum::{routing::get, Json, Router};
use serde::Serialize;
use tokio::sync::oneshot;
use tower_http::{catch_panic::CatchPanicLayer, limit::RequestBodyLimitLayer};
use utoipa::OpenApi;

pub use config::{
    ChannelCacheConfig, Config, ConfigError, DeliveryConfig, IngressConfig, ManagementConfig,
    ServiceAccountConfig, TurnConfig, CMAEG_ACCEPT_BUDGET_MS, CMAEG_ACK_TEXT,
    CMAEG_BUSY_RETRY_MAX_MS, CMAEG_CHANNEL_CACHE_MAX, CMAEG_CHANNEL_CACHE_TTL_MS,
    CMAEG_CHANNEL_NEGATIVE_TTL_MS, CMAEG_CMA_BASE_URL, CMAEG_CMA_SERVICE_TOKEN_FILE,
    CMAEG_COOKIE_KEY_FILE, CMAEG_DRAIN_SECONDS, CMAEG_FAILURE_TEXT, CMAEG_HEARTBEAT_MS,
    CMAEG_INGRESS_INFLIGHT, CMAEG_INGRESS_MAX_BODY, CMAEG_JWKS_TTL_MS, CMAEG_LISTEN,
    CMAEG_MASTER_KEYS_FILE, CMAEG_METRICS_LISTEN, CMAEG_MONGODB_DATABASE, CMAEG_MONGODB_URI,
    CMAEG_NYXID_BASE_URL, CMAEG_NYXID_CLIENT_ID, CMAEG_NYXID_CLIENT_SECRET,
    CMAEG_NYXID_REPLY_TIMEOUT_MS, CMAEG_PROVIDERS, CMAEG_PROVIDER_CONNECT_TIMEOUT_MS,
    CMAEG_PROVIDER_MAX_INFLIGHT, CMAEG_PROVIDER_REQUEST_TIMEOUT_MS, CMAEG_PUBLIC_URL,
    CMAEG_RESTORE_RATE_PER_MINUTE, CMAEG_RESTORE_TIMEOUT_MS, CMAEG_TOOLS_NYXID_SLUG,
    CMAEG_TURN_TIMEOUT_MS, CMAEG_WEB_ORIGIN,
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
    // Outermost backstop: a panic in any middleware is answered too.
    .layer(CatchPanicLayer::custom(http::problems::panic_response))
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
    // Outermost backstop: a panic in any middleware is answered too.
    .layer(CatchPanicLayer::custom(http::problems::panic_response))
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
    let independent_store_enabled = [
        CMAEG_MONGODB_URI,
        CMAEG_MONGODB_DATABASE,
        CMAEG_NYXID_CLIENT_ID,
        CMAEG_NYXID_CLIENT_SECRET,
        CMAEG_WEB_ORIGIN,
        CMAEG_COOKIE_KEY_FILE,
    ]
    .iter()
    .any(|key| std::env::var_os(key).is_some());
    let (records, creator_auth, browser) = if independent_store_enabled {
        let mongo_uri = std::env::var(CMAEG_MONGODB_URI).map_err(|_| {
            format!("{CMAEG_MONGODB_URI} is required when independent Gateway storage is enabled")
        })?;
        let database_name = std::env::var(CMAEG_MONGODB_DATABASE).map_err(|_| {
            format!(
                "{CMAEG_MONGODB_DATABASE} is required when independent Gateway storage is enabled"
            )
        })?;
        let client_id = std::env::var(CMAEG_NYXID_CLIENT_ID).map_err(|_| {
            format!(
                "{CMAEG_NYXID_CLIENT_ID} is required when independent Gateway storage is enabled"
            )
        })?;
        let client_secret = std::env::var(CMAEG_NYXID_CLIENT_SECRET)
            .map_err(|_| format!("{CMAEG_NYXID_CLIENT_SECRET} is required when independent Gateway storage is enabled"))?;
        let web_origin = std::env::var(CMAEG_WEB_ORIGIN).map_err(|_| {
            format!("{CMAEG_WEB_ORIGIN} is required when independent Gateway storage is enabled")
        })?;
        let cookie_key_file = std::env::var(CMAEG_COOKIE_KEY_FILE).map_err(|_| {
            format!(
                "{CMAEG_COOKIE_KEY_FILE} is required when independent Gateway storage is enabled"
            )
        })?;
        let mongo = mongodb::Client::with_uri_str(mongo_uri).await?;
        let database_handle = mongo.database(&database_name);
        let database = records::mongo::MongoRecords::open(database_handle.clone()).await?;
        let service_identity = cma_service_auth::NyxIdAuth::new(
            &config.management.nyxid_base_url,
            cma_service_auth::Service::EventGateway,
            &client_id,
            client_secret,
        )?;
        let browser = std::sync::Arc::new(cma_service_auth::browser::BrowserAuth::new(
            service_identity.clone(),
            &database_handle,
            records::mongo::SESSIONS,
            &web_origin,
            "/api/event-gateway",
            "/event-gateway",
            &std::fs::read(cookie_key_file)?,
        )?);
        let records = std::sync::Arc::new(
            records::RecordClient::new(
                &config.management.cma_base_url,
                config.management.cma_service_token.as_str(),
            )?
            .with_database(database),
        );
        let creator_auth = std::sync::Arc::new(
            auth::CreatorAuthenticator::new(&config.management.nyxid_base_url)?
                .with_service(service_identity),
        );
        (records, creator_auth, Some(browser))
    } else {
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
        (records, creator_auth, None)
    };
    let mut state = http::ManagementState::new(
        providers.clone(),
        records,
        creator_auth,
        config.management.clone(),
        config.channel_cache,
    );
    let telegram = std::sync::Arc::new(telegram::TelegramClient::production(
        config.delivery.reply_timeout,
    )?);
    state.telegram = Some(telegram.clone());
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
        runner.clone(),
    )?
    .with_telegram(telegram)
    .with_interactions(interactions);
    let tools_state = tools::ToolsState {
        resolver: state.resolver.clone(),
        keys: state.master_keys.clone(),
        providers,
        delivery,
        tracker: tracker.clone(),
        public_origin: config.management.public_url.clone(),
    };
    let connections = connections::ConnectionService::new(state.clone(), tools_state.clone());
    let ingress = ingress.with_receipt_observer(std::sync::Arc::new(connections.clone()));
    let _connection_worker = connections.start_completion_worker(runner);
    let listener = tokio::net::TcpListener::bind(config.listen).await?;
    let _metrics =
        http::metrics::MetricsServer::start(config.metrics_listen, telemetry::install()?).await?;
    tracing::info!(bind = %config.listen, "CMAEG listening");

    let (shutdown_started, shutdown_signal) = oneshot::channel();
    let mut router = router_with_state(state)
        .merge(http::events::router(http::events::CustomIngressState {
            ingress: ingress.clone(),
            waiters,
        }))
        .merge(http::connections::router(connections.clone()))
        .merge(http::connection_browser::router(connections))
        .merge(http::telegram::router(ingress.clone()))
        .merge(http::callbacks::router(ingress))
        .merge(http::tools::router(tools_state.clone()))
        .merge(mcp::router(tools_state))
        .layer(RequestBodyLimitLayer::new(REQUEST_BODY_LIMIT_BYTES))
        .layer(CatchPanicLayer::custom(http::problems::panic_response));
    if let Some(browser) = browser {
        router = browser.protect(router).merge(browser.routes());
    }
    let server = axum::serve(
        listener,
        router.into_make_service_with_connect_info::<SocketAddr>(),
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

/// Replace Rust's default panic hook, which prints the panic payload to
/// stderr. A payload can carry request data, so only the location is logged.
pub fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        let location = info
            .location()
            .map(|location| format!("{}:{}", location.file(), location.line()));
        tracing::error!(
            location = %location.as_deref().unwrap_or("unknown"),
            "panic"
        );
    }));
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
