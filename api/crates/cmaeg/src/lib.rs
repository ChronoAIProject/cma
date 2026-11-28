//! CMA Event Gateway HTTP service.
// Production commit and abort go through transaction_terminal (#3299); tests
// may drive the raw driver as independent writers.
#![cfg_attr(test, allow(clippy::disallowed_methods))]

pub mod activity;
mod audit;
pub mod auth;
pub mod channels;
mod config;
pub mod connections;
pub mod console;
pub mod delivery;
pub mod discord;
pub mod github;
pub mod hardening;
pub mod http;
pub mod ingress;
pub mod interactions;
pub mod mcp;
pub mod media;
pub mod migrations;
pub mod native_lark;
pub mod native_media;
pub mod net;
pub mod openapi;
pub mod outbound;
pub mod providers;
mod readback;
pub mod records;
pub mod replicas;
mod request_audit;
pub mod runtime;
mod serve;
mod serve_routed;
mod serve_storage;
mod service_auth;
pub mod telegram;
pub mod telemetry;
pub mod tools;
mod transaction_start;
mod transaction_terminal;
pub mod turns;
pub mod whatsapp;
pub mod worker_credentials;

use axum::{routing::get, Json, Router};
use serde::Serialize;
use tower_http::{catch_panic::CatchPanicLayer, limit::RequestBodyLimitLayer};
use utoipa::OpenApi;

pub use config::{
    ChannelCacheConfig, CommonmarkRelay, Config, ConfigError, ConsoleConfig, ConsoleMemoryConfig,
    DeliveryConfig, IngressConfig, ManagementConfig, MediaConfig, MediaToolProviders,
    NativeDiscordConfig, RoutingConfig, ServiceAccountConfig, SttConfig, SttEndpoint,
    TransformerConfig, TurnConfig, CMAEG_ACCEPT_BUDGET_MS, CMAEG_ACK_TEXT,
    CMAEG_ADVERTISE_PUBLIC_API_URL, CMAEG_BUSY_RETRY_MAX_MS, CMAEG_CHANNEL_CACHE_MAX,
    CMAEG_CHANNEL_CACHE_TTL_MS, CMAEG_CHANNEL_NEGATIVE_TTL_MS, CMAEG_CMA_BASE_URL,
    CMAEG_CMA_SERVICE_TOKEN_FILE, CMAEG_CONSOLE_ACTIVITY_MAX_BYTES,
    CMAEG_CONSOLE_ACTIVITY_PER_CHANNEL, CMAEG_CONSOLE_CHANNEL_IDS_MAX, CMAEG_CONSOLE_CLIENT_ID,
    CMAEG_CONSOLE_CLIENT_SECRET, CMAEG_CONSOLE_ENABLED, CMAEG_CONSOLE_INDEX_MAX_RECORDS,
    CMAEG_CONSOLE_INDEX_RESYNC_MS, CMAEG_CONSOLE_LOGIN_RATE_PER_MINUTE,
    CMAEG_CONSOLE_NYXID_PUBLIC_URL, CMAEG_CONSOLE_PRODUCERS_PER_CHANNEL, CMAEG_CONSOLE_PUBLIC_URL,
    CMAEG_CONSOLE_SESSIONS_MAX, CMAEG_CONSOLE_SESSIONS_PER_SUBJECT, CMAEG_CONSOLE_SESSION_IDLE_MS,
    CMAEG_CONSOLE_SESSION_TTL_MS, CMAEG_CONSOLE_SUMMARY_CACHE_MAX,
    CMAEG_CONSUMER_INTERACTIONS_ENABLED, CMAEG_COOKIE_KEY_FILE, CMAEG_DRAIN_SECONDS,
    CMAEG_FAILURE_TEXT, CMAEG_HEARTBEAT_MS, CMAEG_INGRESS_INFLIGHT, CMAEG_INGRESS_MAX_BODY,
    CMAEG_JWKS_TTL_MS, CMAEG_LISTEN, CMAEG_MASTER_KEYS_FILE, CMAEG_MEDIA_CACHE_BYTES,
    CMAEG_MEDIA_DOWNLOAD_BYTES_PER_HOUR, CMAEG_MEDIA_ENABLED, CMAEG_MEDIA_FFMPEG_DIR,
    CMAEG_MEDIA_INFLIGHT_BYTES, CMAEG_MEDIA_LINK_TTL_SECONDS, CMAEG_MEDIA_MAX_BYTES,
    CMAEG_MEDIA_MAX_JOBS, CMAEG_MEDIA_PREPARE_BUDGET_MS, CMAEG_MEDIA_RATE_PER_MINUTE,
    CMAEG_MEDIA_RESPONSE_BYTES, CMAEG_MEDIA_TOOL_PROVIDERS, CMAEG_MEDIA_VIDEO_FRAMES,
    CMAEG_METRICS_LISTEN, CMAEG_MONGODB_AUTHORITY_MODE, CMAEG_MONGODB_DATABASE, CMAEG_MONGODB_URI,
    CMAEG_NON_HUMAN_PROVIDERS, CMAEG_NYXID_BASE_URL, CMAEG_NYXID_CLIENT_ID,
    CMAEG_NYXID_CLIENT_SECRET, CMAEG_NYXID_COMMONMARK_RELAY_ENABLED, CMAEG_NYXID_COMPLETE_REPLIES,
    CMAEG_NYXID_NATIVE_MEDIA_ENABLED, CMAEG_NYXID_REPLY_TIMEOUT_MS, CMAEG_PROVIDERS,
    CMAEG_PROVIDER_CONNECT_TIMEOUT_MS, CMAEG_PROVIDER_MAX_INFLIGHT,
    CMAEG_PROVIDER_REQUEST_TIMEOUT_MS, CMAEG_PUBLIC_API_URL, CMAEG_PUBLIC_URL,
    CMAEG_RELAY_TEXT_EDITS_ENABLED, CMAEG_RESTORE_RATE_PER_MINUTE, CMAEG_RESTORE_TIMEOUT_MS,
    CMAEG_TELEGRAM_MESSAGE_EFFECTS_ENABLED, CMAEG_TOOLS_NYXID_SLUG, CMAEG_TURN_TIMEOUT_MS,
    CMAEG_WEB_ORIGIN, CMAEG_WHATSAPP_INTERACTIONS_ENABLED, CMAEG_WHATSAPP_TEMPLATES_ENABLED,
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
        probe_router(http::readiness::Readiness::default())
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
    let public_api_url = state.advertised_api_url().to_owned();
    let public_docs = http::docs::router_with_base(&public_api_url, state.advertise_public_api_url);
    let public_discovery = http::public_urls::router(&state);
    let mut document = ApiDoc::openapi();
    document.servers = Some(vec![utoipa::openapi::Server::new(format!(
        "{public_api_url}/v1"
    ))]);
    telemetry::http::instrument(
        http::management::management_router(state)
            .merge(probe_router(readiness))
            .route(
                "/openapi.json",
                get(move || {
                    let document = document.clone();
                    async move { Json(document) }
                }),
            )
            .merge(public_docs)
            .merge(public_discovery)
            .layer(RequestBodyLimitLayer::new(REQUEST_BODY_LIMIT_BYTES))
            .layer(axum::middleware::from_fn(http::problems::protocol_headers)),
    )
    // Outermost backstop: a panic in any middleware is answered too.
    .layer(CatchPanicLayer::custom(http::problems::panic_response))
}

fn probe_router(readiness: http::readiness::Readiness) -> Router {
    // NyxID's /event-gateway/v1 base is rewritten to /v1 by the HTTPRoute.
    // Keep exact public aliases alongside the root infrastructure probes.
    let readyz = get(http::readiness::readyz).with_state(readiness);
    Router::new()
        .route("/healthz", get(healthz))
        .route("/v1/healthz", get(healthz))
        .route("/readyz", readyz.clone())
        .route("/v1/readyz", readyz)
}

async fn healthz() -> Json<StatusResponse> {
    Json(StatusResponse { status: "ok" })
}

async fn openapi() -> Json<utoipa::openapi::OpenApi> {
    Json(ApiDoc::openapi())
}

/// Run CMAEG until a process termination signal arrives.
pub async fn run() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let commonmark_relay = config::CommonmarkRelay::from_env()?;
    let complete_replies = config::nyxid_complete_replies_from_env()?;
    let native_media = config::nyxid_native_media_from_env()?;
    let relay_text_edits = config::relay_text_edits_from_env()?;
    let native_discord_edits = config::native_discord_edits_from_env()?;
    let native_discord_effects = config::native_discord_effects_from_env()?;
    let native_discord_media_effects = config::native_discord_media_effects_from_env()?;
    let native_discord_threads = config::native_discord_thread_metadata_from_env()?;
    let lark_cards = config::lark_cards_from_env()?;
    let lark_forms = config::lark_forms_from_env()?;
    let consumer_interactions = config::consumer_interactions_from_env()?;
    let whatsapp_interactions = config::whatsapp_interactions_from_env()?;
    let discord_interactions = config::discord_interactions_from_env()?;
    let discord_forms = config::discord_forms_from_env()?;
    let discord_card_effects = config::discord_card_effects_from_env()?;
    let discord_card_edits = config::discord_card_edits_from_env()?;
    let discord_multiselect = config::discord_multiselect_from_env()?;
    let whatsapp_templates = config::whatsapp_templates_from_env()?;
    let telegram_effects = config::telegram_message_effects_from_env()?;
    let telegram_media_effects = config::telegram_media_effects_from_env()?;
    let config = Config::from_env()?;
    serve::serve(
        config,
        complete_replies,
        commonmark_relay,
        native_media,
        relay_text_edits,
        native_discord_edits,
        native_discord_effects,
        native_discord_media_effects,
        native_discord_threads,
        lark_cards,
        lark_forms,
        whatsapp_templates,
        whatsapp_interactions,
        discord_interactions,
        discord_forms,
        discord_multiselect,
        discord_card_edits,
        discord_card_effects,
        consumer_interactions,
        telegram_effects,
        telegram_media_effects,
    )
    .await
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
    let writer = telemetry::turn_trace::log_writer();
    tracing_subscriber::registry()
        .with(
            fmt::layer()
                .with_ansi(false)
                .with_writer(writer)
                .with_filter(log_layer_filter(std::env::var("RUST_LOG").ok().as_deref())),
        )
        .with(telemetry::turn_trace::scope::layer())
        .init();
}

/// Initialize the same logger on stderr, for operator commands whose stdout
/// carries a machine-readable report.
pub fn init_logging_to_stderr() {
    use tracing_subscriber::{
        filter::{FilterExt, LevelFilter, Targets},
        fmt,
        layer::SubscriberExt,
        util::SubscriberInitExt,
        Layer,
    };
    let migrations = Targets::new().with_target("cma_service_migrations", LevelFilter::WARN);
    tracing_subscriber::registry()
        .with(
            fmt::layer()
                .with_ansi(false)
                .with_writer(std::io::stderr)
                .with_filter(
                    log_layer_filter(std::env::var("RUST_LOG").ok().as_deref()).or(migrations),
                ),
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
