//! CMA Event Gateway HTTP service.

// Reuse the in-process integration fixtures in socket-free unit tests.
#[cfg(test)]
extern crate self as cmaeg;

mod audit;
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
mod serve;
mod service_auth;
pub mod telegram;
pub mod telemetry;
pub mod tools;
pub mod turns;

use axum::{routing::get, Json, Router};
use serde::Serialize;
use tower_http::{catch_panic::CatchPanicLayer, limit::RequestBodyLimitLayer};
use utoipa::OpenApi;

pub use config::{
    ChannelCacheConfig, Config, ConfigError, DeliveryConfig, IngressConfig, ManagementConfig,
    MediaConfig, MediaToolProviders, RoutingConfig, ServiceAccountConfig, SttConfig, SttEndpoint,
    TransformerConfig, TurnConfig, CMAEG_ACCEPT_BUDGET_MS, CMAEG_ACK_TEXT, CMAEG_BUSY_RETRY_MAX_MS,
    CMAEG_CHANNEL_CACHE_MAX, CMAEG_CHANNEL_CACHE_TTL_MS, CMAEG_CHANNEL_NEGATIVE_TTL_MS,
    CMAEG_CMA_BASE_URL, CMAEG_CMA_SERVICE_TOKEN_FILE, CMAEG_DRAIN_SECONDS, CMAEG_FAILURE_TEXT,
    CMAEG_HEARTBEAT_MS, CMAEG_INGRESS_INFLIGHT, CMAEG_INGRESS_MAX_BODY, CMAEG_JWKS_TTL_MS,
    CMAEG_LISTEN, CMAEG_MASTER_KEYS_FILE, CMAEG_MEDIA_CACHE_BYTES,
    CMAEG_MEDIA_DOWNLOAD_BYTES_PER_HOUR, CMAEG_MEDIA_ENABLED, CMAEG_MEDIA_FFMPEG_DIR,
    CMAEG_MEDIA_INFLIGHT_BYTES, CMAEG_MEDIA_LINK_TTL_SECONDS, CMAEG_MEDIA_MAX_BYTES,
    CMAEG_MEDIA_MAX_JOBS, CMAEG_MEDIA_PREPARE_BUDGET_MS, CMAEG_MEDIA_RATE_PER_MINUTE,
    CMAEG_MEDIA_RESPONSE_BYTES, CMAEG_MEDIA_TOOL_PROVIDERS, CMAEG_MEDIA_VIDEO_FRAMES,
    CMAEG_METRICS_LISTEN, CMAEG_MULTI_CONSUMER_ENABLED, CMAEG_NON_HUMAN_PROVIDERS,
    CMAEG_NYXID_BASE_URL, CMAEG_NYXID_REPLY_TIMEOUT_MS, CMAEG_PROVIDERS,
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

pub use audit::{audit_custom_attachments, audit_routing};
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
    serve::serve(config).await
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
