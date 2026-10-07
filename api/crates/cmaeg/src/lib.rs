//! CMA Event Gateway HTTP service.

pub mod auth;
pub mod channels;
mod config;
pub mod connections;
pub mod delivery;
pub mod hardening;
pub mod http;
pub mod ingress;
pub mod interactions;
pub mod mcp;
pub mod media;
pub mod net;
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
    MediaConfig, MediaToolProviders, ServiceAccountConfig, SttConfig, SttEndpoint, TurnConfig,
    CMAEG_ACCEPT_BUDGET_MS, CMAEG_ACK_TEXT, CMAEG_BUSY_RETRY_MAX_MS, CMAEG_CHANNEL_CACHE_MAX,
    CMAEG_CHANNEL_CACHE_TTL_MS, CMAEG_CHANNEL_NEGATIVE_TTL_MS, CMAEG_CMA_BASE_URL,
    CMAEG_CMA_SERVICE_TOKEN_FILE, CMAEG_DRAIN_SECONDS, CMAEG_FAILURE_TEXT, CMAEG_HEARTBEAT_MS,
    CMAEG_INGRESS_INFLIGHT, CMAEG_INGRESS_MAX_BODY, CMAEG_JWKS_TTL_MS, CMAEG_LISTEN,
    CMAEG_MASTER_KEYS_FILE, CMAEG_MEDIA_CACHE_BYTES, CMAEG_MEDIA_DOWNLOAD_BYTES_PER_HOUR,
    CMAEG_MEDIA_ENABLED, CMAEG_MEDIA_INFLIGHT_BYTES, CMAEG_MEDIA_MAX_BYTES, CMAEG_MEDIA_MAX_JOBS,
    CMAEG_MEDIA_PREPARE_BUDGET_MS, CMAEG_MEDIA_RATE_PER_MINUTE, CMAEG_MEDIA_RESPONSE_BYTES,
    CMAEG_MEDIA_TOOL_PROVIDERS, CMAEG_METRICS_LISTEN, CMAEG_NYXID_BASE_URL,
    CMAEG_NYXID_REPLY_TIMEOUT_MS, CMAEG_PROVIDERS, CMAEG_PROVIDER_CONNECT_TIMEOUT_MS,
    CMAEG_PROVIDER_MAX_INFLIGHT, CMAEG_PROVIDER_REQUEST_TIMEOUT_MS, CMAEG_PUBLIC_URL,
    CMAEG_RESTORE_RATE_PER_MINUTE, CMAEG_RESTORE_TIMEOUT_MS, CMAEG_TOOLS_NYXID_SLUG,
    CMAEG_TURN_TIMEOUT_MS,
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
    let records = std::sync::Arc::new(record_client(&config, service_auth)?);
    let creator_auth = std::sync::Arc::new(auth::CreatorAuthenticator::new(
        &config.management.nyxid_base_url,
    )?);
    let mut state = http::ManagementState::new(
        providers.clone(),
        records,
        creator_auth,
        config.management.clone(),
        config.channel_cache.clone(),
    );
    let telegram = std::sync::Arc::new(telegram::TelegramClient::production(
        config.delivery.reply_timeout,
    )?);
    state.telegram = Some(telegram.clone());
    state.media_enabled = config.media.enabled;
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
    let tool_providers = std::sync::Arc::new(config.media.tool_providers.clone());
    // `None` while CMAEG_MEDIA_ENABLED=false, so nothing is allocated.
    let media = media::service::compose(
        &config,
        media::service::ComposeDeps {
            keys: state.master_keys.clone(),
            tool_providers: tool_providers.clone(),
            telegram: telegram.clone(),
        },
    )?;
    let runner = std::sync::Arc::new(
        turns::TurnRunner::new(
            providers.clone(),
            tracker.clone(),
            delivery.clone(),
            config.turns.clone(),
        )
        .with_media_tool_providers(tool_providers)
        .with_media(
            media,
            media::prepare::PrepareConfig {
                budget: config.media.prepare_budget,
                eager_audio_max_seconds: config.stt.eager_max_seconds,
            },
        ),
    );
    let ingress = ingress::IngressState::new(
        state.resolver.clone(),
        state.master_keys.clone(),
        config.ingress,
        runner.clone(),
    )?
    .with_telegram(telegram)
    .with_interactions(interactions)
    .with_media_enabled(config.media.enabled);
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
    let server = axum::serve(
        listener,
        router_with_state(state)
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
            .layer(CatchPanicLayer::custom(http::problems::panic_response))
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

/// The CMA channel-record client, authenticated as the gateway service.
fn record_client(
    config: &Config,
    service_auth: service_auth::ServiceAuth,
) -> Result<records::RecordClient, records::RecordError> {
    Ok(records::RecordClient::new(
        &config.management.cma_base_url,
        config.management.cma_service_token.as_str(),
    )?
    .with_service_auth(service_auth))
}

/// Audit every stored channel definition for a custom `attachments` setting
/// and, with `clear`, remove it, so an image older than the setting can read
/// every record. It binds no listener and starts no worker. Every error is a
/// classified [`channels::audit::AuditError`], so printing it never reveals a
/// URL, token or upstream body.
pub async fn audit_custom_attachments(
    clear: bool,
) -> Result<channels::audit::AuditReport, Box<dyn std::error::Error + Send + Sync>> {
    use channels::audit::AuditError;
    // Configuration details can name origins; the gateway's own startup log
    // reports them, so the audit only classifies the failure.
    let config = Config::from_env().map_err(|_| AuditError::Configuration)?;
    let service_auth =
        service_auth::ServiceAuth::from_config(&config).map_err(|_| AuditError::ServiceAuth)?;
    // Prove the credential before reading any record, as the gateway does.
    service_auth
        .bearer()
        .await
        .map_err(|_| AuditError::ServiceAuth)?;
    let records = record_client(&config, service_auth).map_err(AuditError::from)?;
    Ok(
        channels::audit::audit_custom_attachments(&records, &config.management.master_keys, clear)
            .await?,
    )
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
    use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, Layer};
    tracing_subscriber::registry()
        .with(
            fmt::layer()
                .with_ansi(false)
                .with_filter(log_layer_filter(std::env::var("RUST_LOG").ok().as_deref())),
        )
        .init();
}

/// Initialize the same logger on stderr, for operator commands whose stdout
/// carries a machine-readable report.
pub fn init_logging_to_stderr() {
    use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, Layer};
    tracing_subscriber::registry()
        .with(
            fmt::layer()
                .with_ansi(false)
                .with_writer(std::io::stderr)
                .with_filter(log_layer_filter(std::env::var("RUST_LOG").ok().as_deref())),
        )
        .init();
}

/// The gateway's log filter: the operator's `RUST_LOG` directives, or
/// `cmaeg=info` when unset or unparsable, combined with a fixed cap that
/// keeps the HTTP client crates at `warn`.
///
/// Their debug and trace events can carry token-bearing URLs, such as a
/// Telegram file URL embedding the bot token. EnvFilter selects the most
/// specific matching directive, so a cap appended to it would lose to
/// `RUST_LOG=reqwest::connect=debug`; only a separate filter combined by AND
/// can hold. `Targets` matches path prefixes, so `reqwest::connect` is capped
/// with `reqwest`. Public so the tests can apply exactly this filter.
pub fn log_layer_filter(
    env: Option<&str>,
) -> impl tracing_subscriber::layer::Filter<tracing_subscriber::Registry> + Send + Sync + 'static {
    use tracing_subscriber::{
        filter::{FilterExt, LevelFilter, Targets},
        EnvFilter,
    };
    let env_filter = env
        .and_then(|directives| EnvFilter::try_new(directives).ok())
        .unwrap_or_else(|| EnvFilter::new(DEFAULT_LOG_FILTER));
    let http_cap = Targets::new()
        .with_default(LevelFilter::TRACE)
        .with_target("reqwest", LevelFilter::WARN)
        .with_target("hyper", LevelFilter::WARN)
        .with_target("hyper_util", LevelFilter::WARN)
        .with_target("h2", LevelFilter::WARN);
    env_filter.and(http_cap)
}
