//! CMA control plane. Wires Layer 1 interfaces onto the Layer 2 core,
//! which drives kernels through Layer 3 adapters.
//!
//! This process never executes agent code. It serves public traffic, keeps one
//! rendered screen per live session, and relays terminals to and from sandboxed
//! runtimes that run elsewhere.

mod activity;
mod auth;
mod chat;
mod config;
mod health;
mod kernels;
mod model_gateway;
mod model_provider;
mod permissions;
mod rate_limit;
mod renewal;
mod screen;
mod skills;
mod sponsorship;
mod store;
mod sweep;
mod trigger;

#[cfg(test)]
mod pty_integration_tests;

#[cfg(test)]
#[path = "rate_limit_tests.rs"]
mod rate_limit_tests;

use std::{future::Future, net::SocketAddr, sync::Arc, time::Duration};

use axum::{
    body::Body,
    extract::{DefaultBodyLimit, Request, State},
    http::{header, HeaderMap, HeaderName, HeaderValue, Method},
    middleware::Next,
    response::Response,
    routing::any,
    Router,
};
use layer1_public::{
    a2a::A2aInterface,
    agui::AguiInterface,
    mcp::McpInterface,
    pty::PtyInterface,
    responses::ResponsesInterface,
    rest::{HttpProblem, RestInterface},
    InterfaceProfile,
};
use layer2_core::{
    ActivityService, Baseline, EnvironmentProfileService, PlatformSettingsService, ScreenModel,
    SessionService, SkillId, SoftwarePackage, TerminalGeometry, TriggerAuthorityVerifier,
    TriggerConnectionManager, TriggerService, UserDirectory,
};
use tower_http::{
    request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer},
    trace::{DefaultOnRequest, DefaultOnResponse, TraceLayer},
};
use tracing::Level;

use crate::activity::ActivityRecorder;
use crate::auth::{AuthState, SignInRecords};
use crate::config::{Config, MongoCutoverStage, MongoCutoverStageConfig};
use crate::screen::VtScreen;

pub(crate) const REQUEST_BODY_LIMIT_BYTES: usize = 2 * 1024 * 1024;
const PROCESS_DRAIN_TIMEOUT: Duration = Duration::from_secs(15);
const DEFAULT_LOG_FILTER: &str = "info";
const X_REQUEST_ID: HeaderName = HeaderName::from_static("x-request-id");
pub(crate) const REST_LINKS: HeaderValue = HeaderValue::from_static(
    "<https://spec.chrono-ai.fun/cma/1.0/conf/rest-core>; rel=\"profile\", <https://spec.chrono-ai.fun/cma/1.0/conf/responses-chat>; rel=\"profile\", </openapi.json>; rel=\"service-desc\"",
);

type TriggerProviderPorts = (
    Arc<dyn TriggerAuthorityVerifier>,
    Arc<dyn TriggerConnectionManager>,
    Option<Arc<trigger::github::GithubBroker>>,
);

/// Composition facts required before a Trigger Launch may claim the native
/// runtime route or any of its closed capabilities.
///
/// Rollout state is deliberately absent. A disabled environment still needs
/// the fully composed scheduler to reconcile retained work and revoke existing
/// reach; rollout policy independently denies new admission and effects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TriggerCompositionReadiness {
    native_runtime_adapter: bool,
    provider_github_broker: bool,
    durable_workspace_store: bool,
    github_capability_signer: bool,
}

impl TriggerCompositionReadiness {
    fn is_ready(self) -> bool {
        self.native_runtime_adapter
            && self.provider_github_broker
            && self.durable_workspace_store
            && self.github_capability_signer
    }

    fn missing_prerequisites(self) -> Vec<&'static str> {
        [
            (
                self.native_runtime_adapter,
                "native_trigger_runtime_adapter",
            ),
            (self.provider_github_broker, "provider_github_broker"),
            (
                self.durable_workspace_store,
                "durable_trigger_workspace_store",
            ),
            (self.github_capability_signer, "github_capability_signer"),
        ]
        .into_iter()
        .filter_map(|(present, name)| (!present).then_some(name))
        .collect()
    }

    fn admitted_session_route(
        self,
        configured_capabilities: &[layer2_core::RequiredCapability],
    ) -> (
        Vec<layer2_core::RequiredCapability>,
        Option<layer2_core::KernelId>,
    ) {
        if self.is_ready() {
            (
                configured_capabilities.to_vec(),
                Some(layer2_core::KernelId::new(trigger::TRIGGER_KERNEL_ID)),
            )
        } else {
            (Vec::new(), None)
        }
    }
}

fn interfaces() -> Vec<Box<dyn InterfaceProfile>> {
    vec![
        Box::new(AguiInterface),
        Box::new(ResponsesInterface),
        Box::new(RestInterface),
        Box::new(PtyInterface),
        Box::new(McpInterface),
        Box::new(A2aInterface),
    ]
}

fn interface_is_served(interface: &dyn InterfaceProfile) -> bool {
    !matches!(interface.interface_id(), "mcp" | "a2a")
}

/// Compose the REST surface before protocol-specific and browser routes join it.
///
/// The Layer 1 routers have different state, so the binary is where they meet.
/// Applying the method fallback after all of them are merged covers every
/// declared REST path without giving any child router a conflicting fallback.
/// The wildcard is path-scoped so `/auth/*` keeps the browser flow's own
/// vocabulary.
fn rest_api_router(
    state: layer1_public::rest::SessionsState,
    admin: Arc<layer1_public::rest::admin::AdminState>,
) -> Router {
    Router::new()
        .merge(layer1_public::rest::complete_router(state, admin))
        .route("/api/v1", any(|| async { HttpProblem::NotFound }))
        .route("/api/v1/", any(|| async { HttpProblem::NotFound }))
        .route("/api/v1/{*path}", any(|| async { HttpProblem::NotFound }))
        .method_not_allowed_fallback(|| async { HttpProblem::MethodNotAllowed })
        .layer(axum::middleware::from_fn(rest_links))
}

/// Keep the private runtime gateway outside middleware that resolves ordinary
/// user credentials. Its per-session bearer has its own exact custody check,
/// body limit, concurrency bound, and timeout inside the gateway router.
fn merge_sponsored_model_gateway(public: Router, gateway: Option<Router>) -> Router {
    match gateway {
        Some(gateway) => public.merge(gateway),
        None => public,
    }
}

/// Advertise the REST profile and its machine-readable service description.
async fn rest_links(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    response.headers_mut().append(header::LINK, REST_LINKS);
    response
}

/// Keep sponsorship responses private even when an outer production guard
/// refuses the request before a sponsorship handler can run.
fn with_sponsorship_privacy(router: Router) -> Router {
    router.layer(axum::middleware::from_fn(sponsorship_privacy))
}

async fn sponsorship_privacy(request: Request, next: Next) -> Response {
    let private = is_sponsorship_path(request.uri().path());
    let mut response = next.run(request).await;
    if private {
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        response.headers_mut().insert(
            header::REFERRER_POLICY,
            HeaderValue::from_static("no-referrer"),
        );
    }
    response
}

fn is_sponsorship_path(path: &str) -> bool {
    sponsorship_span_path(path).is_some()
}

fn sponsorship_span_path(path: &str) -> Option<&'static str> {
    if path == "/internal/v1/sponsored-model/responses" {
        return Some("/internal/v1/sponsored-model/responses");
    }
    if path == "/api/v1/sponsorships" {
        return Some("/api/v1/sponsorships");
    }
    if path.starts_with("/api/v1/sponsorships/") {
        return Some("/api/v1/sponsorships/{redacted}");
    }
    let rest = path.strip_prefix("/api/v1/launches/")?;
    let (_, launch_resource) = rest.split_once('/')?;
    if launch_resource == "sponsorship" {
        return Some("/api/v1/launches/{launch_id}/sponsorship");
    }
    launch_resource
        .split('/')
        .any(|segment| segment == "sponsorship")
        .then_some("/api/v1/launches/{launch_id}/sponsorship/{redacted}")
}

fn request_span_path(path: &str) -> &str {
    sponsorship_span_path(path).unwrap_or(path)
}

/// Apply the browser boundary shared by every public interface.
///
/// Origins remain an explicit deployment grant. A wildcard is incompatible
/// with credentialed requests and would let any site embed this service.
fn with_cors(router: Router, origins: Vec<HeaderValue>) -> Router {
    router.layer(axum::middleware::from_fn_with_state(
        Arc::new(CorsPolicy { origins }),
        cors,
    ))
}

#[derive(Clone)]
struct CorsPolicy {
    origins: Vec<HeaderValue>,
}

async fn cors(State(policy): State<Arc<CorsPolicy>>, request: Request, next: Next) -> Response {
    let origin = request
        .headers()
        .get(header::ORIGIN)
        .filter(|origin| policy.origins.iter().any(|allowed| allowed == *origin))
        .cloned();

    if is_cors_preflight(&request) {
        let mut response = Response::new(Body::empty());
        add_preflight_vary(response.headers_mut());
        if requested_cors_method_is_allowed(request.headers())
            && requested_cors_headers_are_allowed(request.headers())
        {
            if let Some(origin) = origin {
                add_cors_grant(response.headers_mut(), origin);
                response.headers_mut().insert(
                    header::ACCESS_CONTROL_ALLOW_METHODS,
                    HeaderValue::from_static("GET, POST, PUT, PATCH, DELETE"),
                );
                response.headers_mut().insert(
                    header::ACCESS_CONTROL_ALLOW_HEADERS,
                    HeaderValue::from_static(
                        "content-type, authorization, last-event-id, idempotency-key, x-request-id",
                    ),
                );
                response.headers_mut().insert(
                    header::ACCESS_CONTROL_MAX_AGE,
                    HeaderValue::from_static("3600"),
                );
            }
        }
        return response;
    }

    let mut response = next.run(request).await;
    response
        .headers_mut()
        .append(header::VARY, HeaderValue::from_static("origin"));
    if let Some(origin) = origin {
        add_cors_grant(response.headers_mut(), origin);
        response.headers_mut().insert(
            header::ACCESS_CONTROL_EXPOSE_HEADERS,
            HeaderValue::from_static("link, retry-after, x-request-id"),
        );
    }
    response
}

pub(crate) fn is_cors_preflight(request: &Request) -> bool {
    request.method() == Method::OPTIONS
        && request.headers().contains_key(header::ORIGIN)
        && request
            .headers()
            .contains_key(header::ACCESS_CONTROL_REQUEST_METHOD)
}

fn requested_cors_method_is_allowed(headers: &HeaderMap) -> bool {
    headers
        .get(header::ACCESS_CONTROL_REQUEST_METHOD)
        .and_then(|value| Method::from_bytes(value.as_bytes()).ok())
        .is_some_and(|method| {
            matches!(
                method,
                Method::GET | Method::POST | Method::PUT | Method::PATCH | Method::DELETE
            )
        })
}

fn requested_cors_headers_are_allowed(headers: &HeaderMap) -> bool {
    headers
        .get_all(header::ACCESS_CONTROL_REQUEST_HEADERS)
        .iter()
        .all(|value| {
            value.to_str().is_ok_and(|raw| {
                raw.trim().is_empty()
                    || raw.split(',').all(|candidate| {
                        matches!(
                            candidate.trim().to_ascii_lowercase().as_str(),
                            "content-type"
                                | "authorization"
                                | "last-event-id"
                                | "idempotency-key"
                                | "x-request-id"
                        )
                    })
            })
        })
}

fn add_cors_grant(headers: &mut HeaderMap, origin: HeaderValue) {
    headers.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, origin);
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_CREDENTIALS,
        HeaderValue::from_static("true"),
    );
}

fn add_preflight_vary(headers: &mut HeaderMap) {
    for value in [
        "origin",
        "access-control-request-method",
        "access-control-request-headers",
    ] {
        headers.append(header::VARY, HeaderValue::from_static(value));
    }
}

/// Mint or accept a correlation id, put it on the request span, and echo it.
fn with_request_context(router: Router) -> Router {
    router
        // Added in reverse wrapping order: the setter must run before tracing,
        // while propagation must see the response produced by every handler.
        // Validation wraps the setter so an unusable caller value is absent by
        // the time the UUID fallback decides whether it needs to mint one.
        .layer(PropagateRequestIdLayer::new(X_REQUEST_ID))
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(request_span)
                .on_request(DefaultOnRequest::new().level(Level::INFO))
                .on_response(DefaultOnResponse::new().level(Level::INFO)),
        )
        .layer(SetRequestIdLayer::new(X_REQUEST_ID, MakeRequestUuid))
        .layer(axum::middleware::from_fn(discard_unusable_request_id))
}

async fn discard_unusable_request_id(mut request: Request, next: Next) -> Response {
    let normalized = layer1_public::rest::request_id_from_headers(request.headers())
        .and_then(|value| HeaderValue::from_bytes(value.as_bytes()).ok());
    // Remove first so repeated values collapse to either one canonical value or
    // no value; the outer UUID layer mints the latter without ever observing
    // the caller's unusable bytes.
    request.headers_mut().remove(&X_REQUEST_ID);
    if let Some(normalized) = normalized {
        request.headers_mut().insert(&X_REQUEST_ID, normalized);
    }
    next.run(request).await
}

fn request_span(request: &Request) -> tracing::Span {
    let request_id = request
        .headers()
        .get(&X_REQUEST_ID)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("<non-text request id>");
    tracing::info_span!(
        "http.request",
        request_id = %request_id,
        method = %request.method(),
        path = %request_span_path(request.uri().path()),
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MongoCutoverPreparationError {
    LegacyStage,
    Bind,
}

async fn prepare_mongo_cutover_stage<
    Listener,
    RunStage,
    StageFuture,
    StageError,
    Bind,
    BindFuture,
    BindError,
>(
    config: MongoCutoverStageConfig,
    run_stage: RunStage,
    bind: Bind,
) -> Result<Listener, MongoCutoverPreparationError>
where
    RunStage: FnOnce(String, String) -> StageFuture,
    StageFuture: Future<Output = Result<(), StageError>>,
    Bind: FnOnce(String) -> BindFuture,
    BindFuture: Future<Output = Result<Listener, BindError>>,
{
    let MongoCutoverStageConfig {
        stage,
        bind: bind_address,
        mongodb_uri,
        mongodb_database,
    } = config;
    match stage {
        MongoCutoverStage::Legacy => run_stage(mongodb_uri, mongodb_database)
            .await
            .map_err(|_| MongoCutoverPreparationError::LegacyStage)?,
    }
    bind(bind_address)
        .await
        .map_err(|_| MongoCutoverPreparationError::Bind)
}

async fn serve_mongo_cutover_stage(config: MongoCutoverStageConfig) {
    let bind = config.bind.clone();
    // The store wrapper explicitly shuts its MongoDB client down before it
    // returns. The listener bind is sequenced after that return, so successful
    // liveness proves this process is already quiescent at the boundary.
    let listener = match prepare_mongo_cutover_stage(
        config,
        |uri, database| async move {
            crate::store::run_legacy_cutover_stage(&uri, &database).await
        },
        |bind| async move { tokio::net::TcpListener::bind(bind).await },
    )
    .await
    {
        Ok(listener) => listener,
        Err(MongoCutoverPreparationError::LegacyStage) => {
            tracing::error!("legacy MongoDB cutover stage failed; details intentionally omitted");
            std::process::exit(2);
        }
        Err(MongoCutoverPreparationError::Bind) => {
            tracing::error!(
                "cannot bind cutover maintenance listener; details intentionally omitted"
            );
            std::process::exit(2);
        }
    };
    tracing::info!(
        bind = %bind,
        reason = "legacy-cutover-staged",
        "legacy MongoDB cutover staged; public interfaces remain unavailable"
    );
    if axum::serve(
        listener,
        health::legacy_cutover_router().into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await
    .is_err()
    {
        tracing::error!(
            "cutover maintenance server stopped with an error; details intentionally omitted"
        );
        std::process::exit(1);
    }
}

struct StartupBootstrap {
    cutover_stage: Result<Option<MongoCutoverStageConfig>, String>,
    log_filter: tracing_subscriber::EnvFilter,
}

fn bootstrap_startup(
    read_cutover_stage: impl FnOnce() -> Result<Option<MongoCutoverStageConfig>, String>,
    read_normal_log_filter: impl FnOnce() -> tracing_subscriber::EnvFilter,
) -> StartupBootstrap {
    let cutover_stage = read_cutover_stage();
    let log_filter = if matches!(&cutover_stage, Ok(None)) {
        read_normal_log_filter()
    } else {
        // Staged and invalid-stage startup must not let an environment-provided
        // directive expose driver or configuration details before the boundary.
        tracing_subscriber::EnvFilter::new(DEFAULT_LOG_FILTER)
    };
    StartupBootstrap {
        cutover_stage,
        log_filter,
    }
}

async fn dispatch_startup<RunStage, StageFuture, RunNormal, NormalFuture>(
    cutover_stage: Result<Option<MongoCutoverStageConfig>, String>,
    run_stage: RunStage,
    run_normal: RunNormal,
) -> Result<(), String>
where
    RunStage: FnOnce(MongoCutoverStageConfig) -> StageFuture,
    StageFuture: Future<Output = ()>,
    RunNormal: FnOnce() -> NormalFuture,
    NormalFuture: Future<Output = ()>,
{
    match cutover_stage {
        Ok(Some(config)) => run_stage(config).await,
        Ok(None) => run_normal().await,
        Err(error) => return Err(error),
    }
    Ok(())
}

#[tokio::main]
async fn main() {
    let StartupBootstrap {
        cutover_stage,
        log_filter,
    } = bootstrap_startup(MongoCutoverStageConfig::from_env, || {
        tracing_subscriber::EnvFilter::try_from_env("RUST_LOG")
            .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(DEFAULT_LOG_FILTER))
    });
    tracing_subscriber::fmt().with_env_filter(log_filter).init();

    if dispatch_startup(
        cutover_stage,
        serve_mongo_cutover_stage,
        serve_normal_control_plane,
    )
    .await
    .is_err()
    {
        tracing::error!(
            "MongoDB cutover stage configuration is not usable; details intentionally omitted"
        );
        std::process::exit(2);
    }
}

async fn serve_normal_control_plane() {
    let mut config = match Config::from_env() {
        Ok(c) => c,
        Err(e) => {
            // Configuration failures are fatal and must say exactly what is
            // missing; a control plane that starts half-configured is worse
            // than one that refuses to start.
            tracing::error!(error = %e, "configuration is not usable");
            std::process::exit(2);
        }
    };
    // Resolve the exact public gateway origin before constructing Mongo,
    // adapters, workers, or any other sponsorship dependency. Taking the
    // tuple also drops staged-disabled raw secrets at this boundary.
    let sponsorship_gateway_origin = config.sponsorship_gateway_origin();
    let sponsorship_config =
        match sponsorship::select(config.sponsorship.take(), sponsorship_gateway_origin) {
            Ok(selected) => selected,
            Err(error) => {
                tracing::error!(error = %error, "sponsorship configuration is not usable");
                std::process::exit(2);
            }
        };
    tracing::info!(
        bind = %config.bind,
        public_url = %config.public_url,
        sandbox = %config.sandbox_base_url,
        sandbox_image = %config.sandbox_image,
        auth = config.auth_mode_name(),
        rate_admission_per_minute = config.rate_limits.admission_per_minute.get(),
        rate_health_per_minute = config.rate_limits.health_per_minute.get(),
        rate_default_per_minute = config.rate_limits.default_per_minute.get(),
        rate_create_per_minute = config.rate_limits.create_per_minute.get(),
        rate_input_per_minute = config.rate_limits.input_per_minute.get(),
        trusted_proxy_networks = config.trusted_proxies.cidrs.len(),
        trusted_proxy_hops = config.trusted_proxies.forwarded_hops,
        cloudflare_proxy_networks = config.trusted_proxies.cloudflare_cidrs.len(),
        "configuration loaded"
    );

    for iface in interfaces() {
        if interface_is_served(iface.as_ref()) {
            tracing::info!(
                interface = iface.interface_id(),
                operations = iface.operations().len(),
                "interface served"
            );
        } else {
            tracing::info!(
                interface = iface.interface_id(),
                operations = iface.operations().len(),
                "interface declared, not yet served"
            );
        }
    }

    let github_capability_signer = match config.github_capability_signing.as_ref() {
        Some(signing) => {
            match trigger::capability_signing::CapabilitySigner::from_config(signing) {
                Ok(signer) => {
                    tracing::info!(
                        key_id = signer.key_id(),
                        "GitHub runtime-policy signing is enabled"
                    );
                    Some(Arc::new(signer))
                }
                Err(error) => {
                    tracing::error!(error = %error, "GitHub runtime-policy signing is not usable");
                    std::process::exit(2);
                }
            }
        }
        None => {
            tracing::warn!(
                "GitHub runtime-policy signing is disabled; Trigger Launch execution will remain disabled"
            );
            None
        }
    };
    let adapters = crate::kernels::registry(
        &config,
        github_capability_signer
            .as_ref()
            .map(|signer| signer.verification_keys_json())
            .unwrap_or_default(),
    )
    .await;

    let screen_factory: layer2_core::ScreenFactory = Arc::new(|geometry: TerminalGeometry| {
        Box::new(VtScreen::new(geometry)) as Box<dyn ScreenModel>
    });
    // Sessions outlive this process, so the stores are opened before anything
    // is served: starting without them would accept work we cannot keep, and a
    // control plane that opened one and not the other would serve half its
    // surface with the failure found by a user rather than at startup.
    let stores = match crate::store::connect(
        &config.mongodb_uri,
        &config.mongodb_database,
        config.activity_retention,
        config.mongodb_legacy_cutover,
        config.mongodb_coordination_cutover,
        &config.deployment_id,
        &config.auth_encryption_key,
    )
    .await
    {
        Ok(s) => s,
        Err(e) => {
            tracing::error!(error = %e, "cannot open the stores");
            std::process::exit(2);
        }
    };
    // `mongodb::Client::clone` retains the same pool. Keep that handle before
    // moving the ordinary stores out of their startup aggregate.
    let mongo_client = stores.client().clone();
    let store = Arc::new(stores.sessions) as Arc<dyn layer2_core::SessionStore>;
    let profile_store = Arc::new(stores.profiles) as Arc<dyn layer2_core::EnvironmentProfileStore>;
    let settings_store = Arc::new(stores.settings) as Arc<dyn layer2_core::PlatformSettingsStore>;
    let chat_host_store = Arc::new(stores.chat_hosts) as Arc<dyn layer2_core::ChatHostStore>;
    let operation_store = Arc::new(stores.operations) as Arc<dyn layer2_core::OperationStore>;
    let trigger_store = Arc::new(stores.triggers);
    let ticket_store =
        Arc::new(stores.tickets) as Arc<dyn layer1_public::tickets::TerminalTicketStore>;
    let auth_store = Arc::new(stores.auth) as Arc<dyn auth::persistence::AuthStore>;
    // The account of what this service is asked to do, and of who asks. Both
    // are opened with everything else rather than lazily: a control plane that
    // served traffic it could not account for would be discovered by somebody
    // looking for an entry that was never written.
    let activity = ActivityService::new(Arc::new(stores.activity));
    let users = UserDirectory::new(Arc::new(stores.users));
    tracing::info!(
        retention_days = config.activity_retention.as_secs() / 86_400,
        "every request is recorded, and entries older than this are removed"
    );

    // Object storage is optional only for sessions that resolve no skill packs.
    // Without it those sessions still work, but cannot persist a transcript or
    // claim-bound handover snapshot. A session with packs is refused before its
    // row is created because its exact private bytes could neither be recovered
    // nor erased safely.
    //
    // It needs both an address and an identity, and switches off unless both
    // are present. Half a configuration would build a store that authenticates
    // as nobody and fails on first write, long after startup and precisely
    // when a transcript was being kept.
    let identity = crate::store::token::from_config(
        &config.platform_base_url,
        &config.storage_client_id,
        &config.storage_client_secret,
    );
    // The catalogue shares this identity rather than minting a second one:
    // both act as this service against the same platform, and two machine
    // identities for one process is two things to rotate.
    let identity_for_catalog = identity.clone();
    let identity_for_trigger_workspace = identity.clone();
    // The bucket is as load-bearing as the identity: this service writes only
    // into the one it is given and creates none, so an unnamed bucket is not a
    // default to fall back on.
    let storage_base = format!(
        "{}/api/v1/proxy/s/chrono-storage-service",
        config.platform_base_url.trim_end_matches('/')
    );
    let storage = match identity.filter(|_| !config.context_bucket.trim().is_empty()) {
        Some(tokens) => Some((
            Arc::new(crate::store::ChronoStorageContextStore::new(
                storage_base.clone(),
                config.context_bucket.clone(),
                Arc::clone(&tokens),
            )),
            Arc::new(crate::store::ChronoStorageSkillPackStore::new(
                storage_base,
                config.context_bucket.clone(),
                tokens,
            )),
        )),
        None => {
            tracing::warn!(
                "session context, durable handover snapshots, and skill packs will not be stored. \
                 CHRONO_PLATFORM_BASE_URL, \
                 CHRONO_STORAGE_CLIENT_ID, CHRONO_STORAGE_CLIENT_SECRET and \
                 CHRONO_CONTEXT_BUCKET are all required. The bucket must already \
                 exist and be owned by this identity — nothing here creates one. \
                 No other service's credential is valid here. Session creation \
                 will refuse every resolved nonempty skill-pack set until this is configured."
            );
            None
        }
    };
    let context = storage
        .as_ref()
        .map(|(store, _)| Arc::clone(store) as Arc<dyn layer2_core::ContextStore>);
    let handovers = storage
        .as_ref()
        .map(|(store, _)| Arc::clone(store) as Arc<dyn layer2_core::store::HandoverStore>);
    let chat_checkpoints = storage
        .as_ref()
        .map(|(store, _)| Arc::clone(store) as Arc<dyn layer2_core::ChatCheckpointStore>);
    let skill_packs = storage.map(|(_, store)| store as Arc<dyn layer2_core::SkillPackStore>);

    let trigger_workspace_store: Option<Arc<crate::store::ChronoStorageTriggerWorkspaceStore>> =
        match identity_for_trigger_workspace
            .filter(|_| !config.trigger_workspace_bucket.trim().is_empty())
        {
            Some(tokens) => Some(Arc::new(
                crate::store::ChronoStorageTriggerWorkspaceStore::new(
                    format!(
                        "{}/api/v1/proxy/s/chrono-storage-service",
                        config.platform_base_url.trim_end_matches('/')
                    ),
                    config.trigger_workspace_bucket.clone(),
                    tokens,
                    trigger_store.clone() as Arc<dyn layer2_core::LaunchStore>,
                ),
            )),
            None => {
                tracing::warn!(
                    "Trigger repository workspaces cannot be persisted. \
                     CHRONO_PLATFORM_BASE_URL, CHRONO_STORAGE_CLIENT_ID, \
                     CHRONO_STORAGE_CLIENT_SECRET and CHRONO_TRIGGER_WORKSPACE_BUCKET \
                     are all required. No Trigger worker will be started without \
                     this durable store."
                );
                None
            }
        };

    // Built before the service because the service asks it for an owner's
    // credential whenever it has to put a runtime back under a session that
    // nobody is currently watching.
    let auth_state = AuthState::with_store_and_nyxid_public_url(
        config.auth_mode.clone(),
        Some(SignInRecords {
            users: Arc::clone(&users),
            activity: Arc::clone(&activity),
        }),
        auth_store,
        config.nyxid_public_url.clone(),
    );
    let (trigger_authority, trigger_connections, trigger_github): TriggerProviderPorts =
        match &config.auth_mode {
            auth::AuthMode::Provider { base_url, .. } => {
                let broker = match trigger::github::GithubBroker::new(
                    base_url,
                    Arc::clone(&auth_state),
                    trigger_store.clone() as Arc<dyn layer2_core::ConnectionRevocationStore>,
                    config.trigger.public_github_service_slug.clone(),
                ) {
                    Ok(broker) => Arc::new(broker),
                    Err(error) => {
                        tracing::error!(error = %error, "cannot construct the GitHub authority broker");
                        std::process::exit(2);
                    }
                };
                (broker.clone(), broker.clone(), Some(broker))
            }
            auth::AuthMode::Trusted { .. } => {
                let unavailable = Arc::new(trigger::UnavailableGithub);
                (unavailable.clone(), unavailable, None)
            }
        };
    let trigger_composition = TriggerCompositionReadiness {
        native_runtime_adapter: adapters
            .contains_key(&layer2_core::KernelId::new(trigger::TRIGGER_KERNEL_ID)),
        provider_github_broker: trigger_github.is_some(),
        durable_workspace_store: trigger_workspace_store.is_some(),
        github_capability_signer: github_capability_signer.is_some(),
    };
    let (trigger_runtime_capabilities, trigger_kernel) =
        trigger_composition.admitted_session_route(&config.trigger.runtime_capabilities);
    if !auth_state.requires_credential() {
        tracing::warn!(
            "running with a trusted fixed subject; every request is attributed \
             to the same principal. This must never be used outside local development."
        );
    }

    // What every session carries whether or not its owner asked for it. Read
    // from configuration so the list is a deployment fact, while "these cannot
    // be removed" stays a rule of the core.
    let baseline = Baseline::new(
        config.baseline_packages.iter().map(SoftwarePackage::new),
        config.baseline_skills.iter().map(SkillId::new),
        config
            .baseline_services
            .iter()
            .map(layer2_core::ServiceId::new),
    );
    tracing::info!(
        packages = ?config.baseline_packages,
        skills = ?config.baseline_skills,
        "every session carries these, and no profile can remove them"
    );

    // The registry a caller picks skills from. It shares this service's machine
    // identity, and prefers the caller's own credential per request so the
    // answer is what they can see rather than only what is public.
    // Built here rather than after the service, because both the picker that
    // shows what may be chosen and the mint that turns a choice into reach ask
    // the same platform the same question.
    let issuer = (!config.platform_base_url.trim().is_empty()).then(|| {
        Arc::new(auth::keys::KeyIssuer::new(
            reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(15))
                .build()
                .expect("http client builds"),
            config.platform_base_url.clone(),
            config.deployment_id.clone(),
        ))
    });
    let service_picker = issuer.clone().map(|i| {
        Arc::new(auth::keys::ServicePicker::new(i)) as Arc<dyn layer2_core::ServiceCatalog>
    });

    let catalog = crate::skills::from_config(&config.platform_base_url, identity_for_catalog);
    if catalog.is_none() {
        tracing::warn!(
            "no skill registry is configured, so the profile editor will offer \
             nothing to choose from. CHRONO_PLATFORM_BASE_URL, \
             CHRONO_STORAGE_CLIENT_ID and CHRONO_STORAGE_CLIENT_SECRET are all \
             required. Profiles still store and apply skills chosen by other means."
        );
    }

    // One persisted service is shared by every consumer: profile validation,
    // session provisioning and the administrator surface must never observe
    // different revisions of deployment policy by construction.
    let settings = Arc::new(PlatformSettingsService::new(settings_store));
    let environments = EnvironmentProfileService::new_with_reserved_kernels(
        profile_store,
        baseline,
        catalog
            .clone()
            .map(|c| c as Arc<dyn layer2_core::SkillCatalog>),
        service_picker,
        config.default_agent.clone(),
        Arc::clone(&settings),
        [layer2_core::KernelId::new(trigger::TRIGGER_KERNEL_ID)],
    );
    let service = SessionService::new(
        adapters,
        screen_factory,
        Arc::clone(&store),
        layer2_core::SessionPorts {
            context,
            handovers,
            chat_checkpoints,
            credentials: Some(Arc::clone(&auth_state) as Arc<dyn layer2_core::CredentialSource>),
            // The same registry the profile editor picks from, asked for
            // contents rather than for names. A deployment without one still
            // starts sessions; they carry whatever their image does.
            skills: catalog
                .clone()
                .map(|c| c as Arc<dyn layer2_core::SkillSource>),
            workspaces: trigger_workspace_store
                .clone()
                .map(|store| store as Arc<dyn layer2_core::SessionWorkspaceSource>),
            skill_packs,
            settings: Some(Arc::clone(&settings)),
            chat_hosts: Some(chat_host_store),
        },
        Arc::clone(&environments),
        layer2_core::Deployment {
            // The runtime image reference is the runtime version. Deriving it
            // rather than configuring it separately means the two cannot drift:
            // any deployment that changes the image is one whose sessions are
            // carried onto it, and one that does not is not.
            runtime_version: Some(config.sandbox_image.clone()),
            identity: Some(config.deployment_id.clone()),
            adopts_untagged: config.adopts_untagged_runtimes,
            trigger_runtime_capabilities,
            // The versioned Trigger profile selects this adapter explicitly;
            // Layer 2 treats the identifier as opaque and remains kernel-neutral.
            trigger_kernel,
        },
    );
    let mut sponsorship_runtime = match sponsorship::compose(
        sponsorship_config,
        &mongo_client,
        &config.mongodb_database,
        &config.deployment_id,
        Arc::clone(&auth_state),
        Arc::clone(&service),
    )
    .await
    {
        Ok(runtime) => runtime,
        Err(error) => {
            tracing::error!(error = %error, "cannot compose the sponsorship runtime");
            std::process::exit(2);
        }
    };
    let trigger_service = TriggerService::new(
        trigger_store.clone() as Arc<dyn layer2_core::TriggerStore>,
        trigger_store as Arc<dyn layer2_core::LaunchStore>,
        trigger_authority,
    )
    .with_launch_admission(Arc::clone(&service) as Arc<dyn layer2_core::TriggerLaunchAdmission>)
    .with_session_cancellation(
        Arc::clone(&service) as Arc<dyn layer2_core::TriggerSessionCancellation>
    );
    let trigger_service = match sponsorship_runtime.as_ref() {
        Some(sponsorship) => {
            let verifier: Arc<dyn layer2_core::TriggerSponsorshipVerifier> = sponsorship.service();
            trigger_service.with_sponsorship_verifier(verifier)
        }
        None => trigger_service,
    };
    let trigger_service =
        Arc::new(trigger_service.with_rollout(Arc::new(config.trigger.rollout.clone())));
    tracing::info!(
        runtime_version = %config.sandbox_image,
        deployment = %config.deployment_id,
        adopts_untagged = config.adopts_untagged_runtimes,
        "sessions on any other runtime will be moved onto this one"
    );

    // Assembled only after the session service exists: Sandbox is the single
    // creator of the deterministic Trigger session, and Session is its
    // read-only attachment check. No partial worker starts: every closed effect
    // shares one executor, so a missing provider, durable workspace store, or
    // policy signer disables all four scheduler lanes by construction.
    let missing_trigger_worker_prerequisites = trigger_composition.missing_prerequisites();
    let trigger_effect_executor = match (
        trigger_composition.is_ready(),
        trigger_github,
        trigger_workspace_store.as_ref(),
        github_capability_signer.as_ref(),
    ) {
        (true, Some(github), Some(workspace_store), Some(signer)) => {
            let baseline_store: Arc<dyn layer2_core::BaselineObjectStore> =
                Arc::clone(workspace_store) as Arc<dyn layer2_core::BaselineObjectStore>;
            let checkpoint_store: Arc<dyn layer2_core::CheckpointObjectStore> =
                Arc::clone(workspace_store) as Arc<dyn layer2_core::CheckpointObjectStore>;
            let diff_store: Arc<dyn layer2_core::DiffArtifactObjectStore> =
                Arc::clone(workspace_store) as Arc<dyn layer2_core::DiffArtifactObjectStore>;
            let executor = trigger::github_effects::GithubEffectExecutor::with_runtime(
                github,
                baseline_store,
                checkpoint_store,
                diff_store,
                Arc::clone(&service),
                Arc::clone(signer),
                trigger::github_effects::GithubEffectRuntimeLimits {
                    fork_readiness_timeout: config.trigger.fork_readiness_timeout,
                    changed_file_limit: config.trigger.changed_file_limit,
                },
            );
            let executor = match sponsorship_runtime.as_ref() {
                Some(sponsorship) => {
                    executor.with_sponsorship(sponsorship.service(), sponsorship.gateway_origin())
                }
                None => executor,
            };
            Some(Arc::new(executor))
        }
        _ => None,
    };

    // Trigger's Session effect verifies exact delegated services. Install the
    // issuer before any scheduler lane is spawned because Tokio intervals tick
    // immediately and a restart must not spend an effect attempt against a
    // transiently incomplete authentication runtime.
    if let Some(issuer) = issuer {
        auth_state.use_issuer(issuer).await;
        tracing::info!(
            deployment = %config.deployment_id,
            services = ?config.baseline_services,
            "each session gets its own credential, reaching these services and whatever its profile adds"
        );
    }

    match trigger_effect_executor {
        Some(executor) => {
            if let Err(error) = trigger::scheduler::spawn(
                Arc::clone(&trigger_service),
                executor,
                Arc::clone(&service),
                Arc::clone(&activity),
                &config.deployment_id,
                &config.trigger,
            ) {
                tracing::error!(
                    error = %error,
                    "cannot construct the fenced Trigger scheduler"
                );
                std::process::exit(2);
            }
        }
        None => tracing::warn!(
            missing_prerequisites = ?missing_trigger_worker_prerequisites,
            "Trigger provider workers are disabled"
        ),
    }

    renewal::spawn(Arc::clone(&auth_state), Arc::clone(&service));

    // Recovery-point behavior and host claims belong to the core. This binary
    // owns only the deployment cadence that asks for one bounded pass.
    chat::checkpoints::spawn(Arc::clone(&service));
    let chat_title_generator = chat::titles::HttpChatTitleGenerator::new(
        &config.upstream_base_url,
        &config.upstream_model,
    )
    .map(|generator| Arc::new(generator) as Arc<dyn layer2_core::ChatTitleGenerator>);
    if chat_title_generator.is_none() {
        tracing::warn!(
            "chat title model adapter is unavailable; deterministic title fallback remains active"
        );
    }
    chat::sweeper::spawn(Arc::clone(&service), chat_title_generator);

    // Sessions end only when their owner says so, which leaves this as the one
    // thing that reclaims a runtime nothing refers to or finishes a session
    // whose start-up died with an earlier process.
    sweep::spawn(Arc::clone(&service));

    // What somebody running the service can see and do. Its own router under
    // its own path segment so the whole surface can be restricted in front of
    // this process as well as inside it.
    // Minted where a credential can be carried and redeemed where one cannot,
    // so both surfaces are handed the same store.
    let tickets = layer1_public::tickets::TerminalTickets::new(ticket_store);

    let public_state = layer1_public::rest::SessionsState {
        sessions: Arc::clone(&service),
        users: Arc::clone(&users),
        tickets: Arc::clone(&tickets),
        environments: Arc::clone(&environments),
        operations: Arc::clone(&operation_store),
        triggers: Some(layer1_public::rest::TriggersState {
            triggers: Arc::clone(&trigger_service),
            connections: trigger_connections,
            operations: Arc::clone(&operation_store),
            diff_artifacts: trigger_workspace_store
                .clone()
                .map(|store| store as Arc<dyn layer2_core::DiffArtifactObjectStore>),
            sponsorships: sponsorship_runtime
                .as_ref()
                .map(sponsorship::SponsorshipRuntime::service),
        }),
    };
    let rest = rest_api_router(
        public_state.clone(),
        Arc::new(layer1_public::rest::admin::AdminState {
            sessions: Arc::clone(&service),
            activity: Arc::clone(&activity),
            users: Arc::clone(&users),
            settings: Arc::clone(&settings),
            triggers: Some(trigger_service),
            operations: operation_store,
        }),
    );
    let rate_limits =
        rate_limit::RateLimiters::from_config(config.rate_limits, config.trusted_proxies.clone());
    let sponsored_model_gateway = sponsorship_runtime
        .as_ref()
        .map(sponsorship::SponsorshipRuntime::gateway_router)
        .map(|gateway| {
            gateway.layer(axum::middleware::from_fn_with_state(
                Arc::clone(&rate_limits),
                rate_limit::admit,
            ))
        });
    let public = Router::new()
        .merge(health::router(store))
        .merge(rest)
        // The priority-1 interface, sharing the facade's state because it
        // projects the same sessions: a thread here is a session there, and
        // two states would be two answers to the same question.
        .merge(layer1_public::agui::router(public_state))
        .merge(layer1_public::pty::router(
            layer1_public::pty::TerminalState {
                sessions: Arc::clone(&service),
                tickets: Arc::clone(&tickets),
            },
        ))
        .merge(layer1_public::mcp::router())
        .merge(layer1_public::a2a::router())
        .merge(auth::router(Arc::clone(&auth_state)))
        // Added in reverse wrapping order. Activity records admitted work,
        // never the traffic being refused to protect its own Mongo write.
        .layer(axum::middleware::from_fn_with_state(
            ActivityRecorder::new(Arc::clone(&activity)),
            activity::record,
        ))
        // Exact route-class limits see the identity established outside them
        // and run before activity buffers a body or writes MongoDB.
        .layer(axum::middleware::from_fn_with_state(
            Arc::clone(&rate_limits),
            rate_limit::enforce,
        ))
        .layer(axum::middleware::from_fn_with_state(
            Arc::clone(&auth_state),
            auth::attach_subject,
        ))
        // A cheap network guard is outside authentication, bounding store
        // lookups and bearer introspection as well as downstream work.
        .layer(axum::middleware::from_fn_with_state(
            rate_limits,
            rate_limit::admit,
        ))
        // The extension is outermost so every body extractor sees the chosen
        // ceiling. The activity recorder uses the same constant.
        .layer(DefaultBodyLimit::max(REQUEST_BODY_LIMIT_BYTES));
    // Merge only after public middleware is fixed onto its routes. The model
    // gateway must authenticate the runtime bearer itself, not send it through
    // ordinary subject authentication or provider introspection.
    let routed = merge_sponsored_model_gateway(public, sponsored_model_gateway);
    let app = with_sponsorship_privacy(with_request_context(with_cors(
        routed,
        config.cors_origins.clone(),
    )));

    let listener = match tokio::net::TcpListener::bind(&config.bind).await {
        Ok(l) => l,
        Err(e) => {
            tracing::error!(bind = %config.bind, error = %e, "cannot bind");
            std::process::exit(2);
        }
    };
    tracing::info!(bind = %config.bind, "control plane listening");

    let (shutdown_notice, shutdown_wait) = tokio::sync::watch::channel(false);
    let sponsorship_workers = sponsorship_runtime
        .as_mut()
        .and_then(|runtime| runtime.start_workers(shutdown_notice.clone()));
    let server = async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(wait_for_shutdown(shutdown_wait))
        .await
    };
    tokio::pin!(server);

    // A server failure is handled immediately. A termination signal instead
    // starts one absolute budget for both Axum and the owned recovery lanes;
    // neither drain is allowed to consume the other's Kubernetes grace period.
    let server_completed = tokio::select! {
        result = server.as_mut() => Some(result),
        _ = shutdown_signal() => None,
    };
    let _ = shutdown_notice.send(true);
    let server_result = match server_completed {
        Some(result) => {
            if let Some(workers) = sponsorship_workers {
                workers.shutdown().await;
            }
            result
        }
        None => {
            let worker_drain = async move {
                if let Some(workers) = sponsorship_workers {
                    workers.shutdown().await;
                }
            };
            match tokio::time::timeout(PROCESS_DRAIN_TIMEOUT, async {
                let (server_result, ()) = tokio::join!(server.as_mut(), worker_drain);
                server_result
            })
            .await
            {
                Ok(result) => result,
                Err(_) => {
                    tracing::warn!(
                        timeout_seconds = PROCESS_DRAIN_TIMEOUT.as_secs(),
                        "process drain timed out; cancelling unfinished HTTP and recovery work"
                    );
                    return;
                }
            }
        }
    };
    if let Err(e) = server_result {
        tracing::error!(error = %e, "server stopped with an error");
        std::process::exit(1);
    }
}

async fn wait_for_shutdown(mut shutdown: tokio::sync::watch::Receiver<bool>) {
    while !*shutdown.borrow() {
        if shutdown.changed().await.is_err() {
            break;
        }
    }
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("ctrl-c handler installs");
    };
    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("SIGTERM handler installs")
            .recv()
            .await;
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
    tracing::info!("shutdown signal received; draining");
}

#[cfg(test)]
#[path = "rest_contract_tests.rs"]
mod rest_contract_tests;

#[cfg(test)]
#[path = "interface_contract_tests.rs"]
mod interface_contract_tests;

#[cfg(test)]
mod tests {
    use std::sync::{atomic::AtomicUsize, atomic::Ordering, Mutex};

    use super::*;

    fn legacy_stage_config() -> MongoCutoverStageConfig {
        MongoCutoverStageConfig {
            stage: MongoCutoverStage::Legacy,
            bind: "127.0.0.1:18080".into(),
            mongodb_uri: "mongodb://stage.invalid/cma".into(),
            mongodb_database: "stage_database".into(),
        }
    }

    #[test]
    fn startup_resolves_the_stage_before_reading_the_normal_log_filter() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let stage_events = Arc::clone(&events);
        let log_events = Arc::clone(&events);

        let bootstrap = bootstrap_startup(
            move || {
                stage_events.lock().unwrap().push("cutover-stage");
                Ok(None)
            },
            move || {
                log_events.lock().unwrap().push("rust-log");
                tracing_subscriber::EnvFilter::new("warn")
            },
        );

        assert!(matches!(bootstrap.cutover_stage, Ok(None)));
        assert_eq!(bootstrap.log_filter.to_string(), "warn");
        assert_eq!(*events.lock().unwrap(), ["cutover-stage", "rust-log"]);
    }

    #[test]
    fn staged_and_invalid_stage_startup_never_reads_rust_log() {
        for cutover_stage in [
            Ok(Some(legacy_stage_config())),
            Err("invalid nonempty cutover stage".to_owned()),
        ] {
            let rust_log_reads = AtomicUsize::new(0);
            let bootstrap = bootstrap_startup(
                move || cutover_stage,
                || {
                    rust_log_reads.fetch_add(1, Ordering::SeqCst);
                    tracing_subscriber::EnvFilter::new("trace")
                },
            );

            assert_eq!(rust_log_reads.load(Ordering::SeqCst), 0);
            assert_eq!(bootstrap.log_filter.to_string(), "info");
        }
    }

    #[tokio::test]
    async fn selected_stage_never_enters_normal_configuration_or_provider_startup() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let stage_calls = Arc::clone(&calls);
        let normal_calls = Arc::clone(&calls);

        dispatch_startup(
            Ok(Some(legacy_stage_config())),
            move |config| {
                assert_eq!(config.stage, MongoCutoverStage::Legacy);
                stage_calls.lock().unwrap().push("legacy-cutover");
                std::future::ready(())
            },
            move || async move {
                normal_calls.lock().unwrap().extend([
                    "normal-config",
                    "kernel-registry-and-sandbox",
                    "mongodb-stores",
                    "storage-and-skill-catalog",
                    "nyxid-and-auth",
                    "renewal-and-sweep",
                    "public-listener",
                ]);
                panic!("normal configuration and providers are deliberately poisoned")
            },
        )
        .await
        .expect("the selected stage completes");

        assert_eq!(*calls.lock().unwrap(), ["legacy-cutover"]);
    }

    #[tokio::test]
    async fn invalid_stage_enters_neither_startup_branch() {
        let calls = Arc::new(AtomicUsize::new(0));
        let stage_calls = Arc::clone(&calls);
        let normal_calls = Arc::clone(&calls);

        let error = dispatch_startup(
            Err("invalid nonempty cutover stage".to_owned()),
            move |_| {
                stage_calls.fetch_add(1, Ordering::SeqCst);
                std::future::ready(())
            },
            move || {
                normal_calls.fetch_add(1, Ordering::SeqCst);
                std::future::ready(())
            },
        )
        .await
        .expect_err("an invalid selector cannot choose a startup branch");

        assert_eq!(error, "invalid nonempty cutover stage");
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn sponsored_model_gateway_bypasses_ordinary_auth_middleware() {
        use axum::http::StatusCode;
        use axum::routing::post;
        use tower::ServiceExt as _;

        let public_auth_calls = Arc::new(AtomicUsize::new(0));
        let observed_auth_calls = Arc::clone(&public_auth_calls);
        let public = Router::new()
            .route("/public", post(|| async { StatusCode::NO_CONTENT }))
            .layer(axum::middleware::from_fn(
                move |request: Request, next: Next| {
                    let observed_auth_calls = Arc::clone(&observed_auth_calls);
                    async move {
                        observed_auth_calls.fetch_add(1, Ordering::SeqCst);
                        next.run(request).await
                    }
                },
            ));
        let gateway = Router::new().route(
            "/internal/v1/sponsored-model/responses",
            post(|| async { StatusCode::NO_CONTENT }),
        );
        let app = merge_sponsored_model_gateway(public, Some(gateway));

        let gateway_response = app
            .clone()
            .oneshot(
                Request::post("/internal/v1/sponsored-model/responses")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(gateway_response.status(), StatusCode::NO_CONTENT);
        assert_eq!(public_auth_calls.load(Ordering::SeqCst), 0);

        let public_response = app
            .oneshot(Request::post("/public").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(public_response.status(), StatusCode::NO_CONTENT);
        assert_eq!(public_auth_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn legacy_stage_completion_and_client_shutdown_precede_listener_bind() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let stage_events = Arc::clone(&events);
        let bind_events = Arc::clone(&events);

        let listener = prepare_mongo_cutover_stage(
            legacy_stage_config(),
            move |uri, database| async move {
                assert_eq!(uri, "mongodb://stage.invalid/cma");
                assert_eq!(database, "stage_database");
                stage_events.lock().unwrap().push("stage-complete");
                tokio::task::yield_now().await;
                stage_events.lock().unwrap().push("client-shutdown");
                Ok::<(), ()>(())
            },
            move |bind| async move {
                assert_eq!(bind, "127.0.0.1:18080");
                bind_events.lock().unwrap().push("listener-bind");
                Ok::<_, ()>("maintenance-listener")
            },
        )
        .await
        .expect("the maintenance boundary prepares");

        assert_eq!(listener, "maintenance-listener");
        assert_eq!(
            *events.lock().unwrap(),
            ["stage-complete", "client-shutdown", "listener-bind"]
        );
    }

    #[tokio::test]
    async fn a_failed_legacy_stage_never_binds_the_listener() {
        let bind_calls = Arc::new(AtomicUsize::new(0));
        let observed_bind_calls = Arc::clone(&bind_calls);

        let error = prepare_mongo_cutover_stage(
            legacy_stage_config(),
            |_, _| std::future::ready(Err::<(), _>("deliberate stage failure")),
            move |_| {
                observed_bind_calls.fetch_add(1, Ordering::SeqCst);
                std::future::ready(Ok::<_, ()>(()))
            },
        )
        .await
        .expect_err("a failed migration cannot expose maintenance liveness");

        assert_eq!(error, MongoCutoverPreparationError::LegacyStage);
        assert_eq!(bind_calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn request_span_redacts_sponsorship_paths_and_query_credentials() {
        use tracing::{field::Visit, span::Attributes, Subscriber};
        use tracing_subscriber::{layer::Context, prelude::*, registry::LookupSpan, Layer};

        #[derive(Clone)]
        struct Capture(Arc<std::sync::Mutex<Vec<(String, String)>>>);

        struct Fields<'a>(&'a std::sync::Mutex<Vec<(String, String)>>);

        impl Visit for Fields<'_> {
            fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
                self.0
                    .lock()
                    .expect("capture lock is usable")
                    .push((field.name().to_string(), format!("{value:?}")));
            }
        }

        impl<S> Layer<S> for Capture
        where
            S: Subscriber + for<'lookup> LookupSpan<'lookup>,
        {
            fn on_new_span(
                &self,
                attributes: &Attributes<'_>,
                _id: &tracing::span::Id,
                _context: Context<'_, S>,
            ) {
                attributes.record(&mut Fields(&self.0));
            }
        }

        let fields = Arc::new(std::sync::Mutex::new(Vec::new()));
        let subscriber = tracing_subscriber::registry().with(Capture(Arc::clone(&fields)));
        tracing::subscriber::with_default(subscriber, || {
            let ordinary_request = Request::get(
                "/api/v1/sessions/sesn_trace_canary/terminal\
                 ?ticket=pty-ticket-secret-488&code=oauth-code-secret-488\
                 &state=oauth-state-secret-488",
            )
            .header("x-request-id", "48800000-0000-4000-8000-000000000006")
            .body(axum::body::Body::empty())
            .expect("request builds");
            let _ordinary_span = request_span(&ordinary_request);
            let sponsorship_request = Request::get(
                "/api/v1/sponsorships/credentials/credential-path-marker-488/\
                 malformed-suffix-marker-488?cursor=sponsorship-query-marker-488",
            )
            .body(axum::body::Body::empty())
            .expect("sponsorship request builds");
            let _sponsorship_span = request_span(&sponsorship_request);
            let launch_request = Request::get(
                "/api/v1/launches/launch-path-marker-488/sponsorship/\
                 malformed-launch-suffix-marker-488?cursor=launch-query-marker-488",
            )
            .body(axum::body::Body::empty())
            .expect("Launch sponsorship request builds");
            let _launch_span = request_span(&launch_request);
            let empty_launch_request = Request::get(
                "/api/v1/launches//sponsorship/malformed-empty-launch-marker-488\
                 ?cursor=empty-launch-query-marker-488",
            )
            .body(axum::body::Body::empty())
            .expect("empty Launch sponsorship request builds");
            let _empty_launch_span = request_span(&empty_launch_request);
            let repeated_separator_request = Request::get(
                "/api/v1/launches/repeated-launch-marker-488//sponsorship/\
                 malformed-repeated-marker-488?cursor=repeated-launch-query-marker-488",
            )
            .body(axum::body::Body::empty())
            .expect("repeated-separator sponsorship request builds");
            let _repeated_separator_span = request_span(&repeated_separator_request);
            let ordinary_launch_request =
                Request::get("/api/v1/launches/ordinary-launch-control-489/status")
                    .body(axum::body::Body::empty())
                    .expect("ordinary Launch request builds");
            let _ordinary_launch_span = request_span(&ordinary_launch_request);
        });
        let fields = fields.lock().expect("capture lock is usable");
        assert!(
            fields.iter().any(|(name, value)| name == "request_id"
                && value.contains("48800000-0000-4000-8000-000000000006")),
            "request span must carry the support correlation id: {fields:?}"
        );
        assert!(
            fields.iter().any(|(name, value)| {
                name == "path" && value.contains("/api/v1/sessions/sesn_trace_canary/terminal")
            }),
            "request span must carry the request path: {fields:?}"
        );
        assert!(
            fields.iter().any(|(name, value)| {
                name == "path" && value.contains("/api/v1/sponsorships/{redacted}")
            }),
            "sponsorship spans must use a fixed redacted path: {fields:?}"
        );
        assert!(
            fields.iter().any(|(name, value)| {
                name == "path"
                    && value.contains("/api/v1/launches/{launch_id}/sponsorship/{redacted}")
            }),
            "Launch sponsorship spans must redact the Launch and suffix: {fields:?}"
        );
        assert!(
            fields.iter().any(|(name, value)| {
                name == "path"
                    && value.contains("/api/v1/launches/ordinary-launch-control-489/status")
            }),
            "ordinary Launch spans must preserve their request path: {fields:?}"
        );
        for secret in [
            "pty-ticket-secret-488",
            "oauth-code-secret-488",
            "oauth-state-secret-488",
            "credential-path-marker-488",
            "malformed-suffix-marker-488",
            "sponsorship-query-marker-488",
            "launch-path-marker-488",
            "malformed-launch-suffix-marker-488",
            "launch-query-marker-488",
            "malformed-empty-launch-marker-488",
            "empty-launch-query-marker-488",
            "repeated-launch-marker-488",
            "malformed-repeated-marker-488",
            "repeated-launch-query-marker-488",
        ] {
            assert!(
                fields.iter().all(|(_, value)| !value.contains(secret)),
                "request span leaked query credential {secret}: {fields:?}"
            );
        }
        assert!(
            fields.iter().all(|(name, _)| name != "uri"),
            "request span must not imply that it records a full URI: {fields:?}"
        );
    }

    #[test]
    fn every_interface_has_a_unique_id() {
        let ids: Vec<_> = interfaces().iter().map(|i| i.interface_id()).collect();
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(ids.len(), sorted.len(), "interface ids must be unique");
    }

    #[test]
    fn every_interface_projects_at_least_one_operation() {
        for iface in interfaces() {
            assert!(
                !iface.operations().is_empty(),
                "{} projects no operations",
                iface.interface_id()
            );
        }
    }

    #[test]
    fn delegated_issuer_is_installed_before_trigger_scheduler_spawn() {
        let source = include_str!("main.rs");
        let startup = source
            .split_once("#[cfg(test)]\nmod tests")
            .expect("main keeps one test module after startup")
            .0;
        let issuer = startup
            .find("auth_state.use_issuer(issuer).await;")
            .expect("normal startup installs the delegated issuer");
        let scheduler = startup
            .find("trigger::scheduler::spawn(")
            .expect("normal startup constructs the Trigger scheduler");
        assert!(
            issuer < scheduler,
            "an immediate scheduler tick must observe the installed issuer"
        );
    }

    #[test]
    fn trigger_route_and_workers_share_one_complete_prerequisite_table() {
        let complete = TriggerCompositionReadiness {
            native_runtime_adapter: true,
            provider_github_broker: true,
            durable_workspace_store: true,
            github_capability_signer: true,
        };
        let configured = vec![
            layer2_core::RequiredCapability::Conversation,
            layer2_core::RequiredCapability::RestrictedServiceGateway,
        ];
        let (capabilities, kernel) = complete.admitted_session_route(&configured);
        assert_eq!(capabilities, configured);
        assert_eq!(kernel.unwrap().as_str(), trigger::TRIGGER_KERNEL_ID);
        assert!(complete.missing_prerequisites().is_empty());

        for (missing, expected) in [
            (
                TriggerCompositionReadiness {
                    native_runtime_adapter: false,
                    ..complete
                },
                "native_trigger_runtime_adapter",
            ),
            (
                TriggerCompositionReadiness {
                    provider_github_broker: false,
                    ..complete
                },
                "provider_github_broker",
            ),
            (
                TriggerCompositionReadiness {
                    durable_workspace_store: false,
                    ..complete
                },
                "durable_trigger_workspace_store",
            ),
            (
                TriggerCompositionReadiness {
                    github_capability_signer: false,
                    ..complete
                },
                "github_capability_signer",
            ),
        ] {
            let (capabilities, kernel) = missing.admitted_session_route(&configured);
            assert!(capabilities.is_empty(), "{expected}");
            assert!(kernel.is_none(), "{expected}");
            assert_eq!(missing.missing_prerequisites(), [expected]);
        }
    }

    #[test]
    fn incomplete_trigger_composition_reports_every_missing_boundary_in_stable_order() {
        let missing = TriggerCompositionReadiness {
            native_runtime_adapter: false,
            provider_github_broker: false,
            durable_workspace_store: false,
            github_capability_signer: false,
        };
        assert_eq!(
            missing.missing_prerequisites(),
            [
                "native_trigger_runtime_adapter",
                "provider_github_broker",
                "durable_trigger_workspace_store",
                "github_capability_signer",
            ]
        );
    }
}
