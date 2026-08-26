//! CMA control plane. Wires Layer 1 interfaces onto the Layer 2 core,
//! which drives kernels through Layer 3 adapters.
//!
//! This process never executes agent code. It serves public traffic, keeps one
//! rendered screen per live session, and relays terminals to and from sandboxed
//! runtimes that run elsewhere.

mod activity;
mod auth;
mod config;
mod health;
mod kernels;
mod permissions;
mod rate_limit;
mod renewal;
mod screen;
mod skills;
mod store;
mod sweep;
mod trigger;

#[cfg(test)]
mod pty_integration_tests;

#[cfg(test)]
#[path = "rate_limit_tests.rs"]
mod rate_limit_tests;

use std::{net::SocketAddr, sync::Arc};

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
use crate::config::Config;
use crate::screen::VtScreen;

pub(crate) const REQUEST_BODY_LIMIT_BYTES: usize = 2 * 1024 * 1024;
const X_REQUEST_ID: HeaderName = HeaderName::from_static("x-request-id");
pub(crate) const REST_LINKS: HeaderValue = HeaderValue::from_static(
    "<https://spec.chrono-ai.fun/cma/1.0/conf/rest-core>; rel=\"profile\", </openapi.json>; rel=\"service-desc\"",
);

fn interfaces() -> Vec<Box<dyn InterfaceProfile>> {
    vec![
        Box::new(AguiInterface),
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
/// The three Layer 1 routers have different state, so the binary is where they
/// meet. Applying the method fallback after all three are merged covers every
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

/// Advertise the REST profile and its machine-readable service description.
async fn rest_links(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    response.headers_mut().append(header::LINK, REST_LINKS);
    response
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
    let usable = request
        .headers()
        .get(&X_REQUEST_ID)
        .is_none_or(|value| value.to_str().is_ok_and(|text| !text.trim().is_empty()));
    if !usable {
        request.headers_mut().remove(&X_REQUEST_ID);
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
        path = %request.uri().path(),
    )
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("RUST_LOG")
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let config = match Config::from_env() {
        Ok(c) => c,
        Err(e) => {
            // Configuration failures are fatal and must say exactly what is
            // missing; a control plane that starts half-configured is worse
            // than one that refuses to start.
            tracing::error!(error = %e, "configuration is not usable");
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
    );

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
    let store = Arc::new(stores.sessions) as Arc<dyn layer2_core::SessionStore>;
    let profile_store = Arc::new(stores.profiles) as Arc<dyn layer2_core::EnvironmentProfileStore>;
    let settings_store = Arc::new(stores.settings) as Arc<dyn layer2_core::PlatformSettingsStore>;
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

    // Context storage is optional: without it a session still works, it simply
    // cannot carry its transcript into a new sandbox.
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
    let context: Option<Arc<dyn layer2_core::ContextStore>> =
        match identity.filter(|_| !config.context_bucket.trim().is_empty()) {
            Some(tokens) => Some(Arc::new(crate::store::ChronoStorageContextStore::new(
                format!(
                    "{}/api/v1/proxy/s/chrono-storage-service",
                    config.platform_base_url.trim_end_matches('/')
                ),
                config.context_bucket.clone(),
                tokens,
            ))),
            None => {
                tracing::warn!(
                    "session context will not be stored. CHRONO_PLATFORM_BASE_URL, \
                 CHRONO_STORAGE_CLIENT_ID, CHRONO_STORAGE_CLIENT_SECRET and \
                 CHRONO_CONTEXT_BUCKET are all required. The bucket must already \
                 exist and be owned by this identity — nothing here creates one. \
                 No other service's credential is valid here."
                );
                None
            }
        };

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
    let auth_state = AuthState::with_store(
        config.auth_mode.clone(),
        Some(SignInRecords {
            users: Arc::clone(&users),
            activity: Arc::clone(&activity),
        }),
        auth_store,
    );
    let (trigger_authority, trigger_connections, trigger_github): (
        Arc<dyn TriggerAuthorityVerifier>,
        Arc<dyn TriggerConnectionManager>,
        Option<Arc<trigger::github::GithubBroker>>,
    ) = match &config.auth_mode {
        auth::AuthMode::Provider { base_url, .. } => {
            let broker = match trigger::github::GithubBroker::new(base_url, Arc::clone(&auth_state))
            {
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
    let trigger_service = Arc::new(TriggerService::new(
        trigger_store.clone() as Arc<dyn layer2_core::TriggerStore>,
        trigger_store as Arc<dyn layer2_core::LaunchStore>,
        trigger_authority,
    ));
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
    let environments = EnvironmentProfileService::new(
        profile_store,
        baseline,
        catalog
            .clone()
            .map(|c| c as Arc<dyn layer2_core::SkillCatalog>),
        service_picker,
        config.default_agent.clone(),
        Arc::clone(&settings),
    );
    let service = SessionService::new(
        adapters,
        screen_factory,
        Arc::clone(&store),
        layer2_core::SessionPorts {
            context,
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
            settings: Some(Arc::clone(&settings)),
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
        },
    );
    tracing::info!(
        runtime_version = %config.sandbox_image,
        deployment = %config.deployment_id,
        adopts_untagged = config.adopts_untagged_runtimes,
        "sessions on any other runtime will be moved onto this one"
    );

    // Assembled only after the session service exists: Sandbox is the single
    // creator of the deterministic Trigger session, and Session is its
    // read-only attachment check. Scheduler activation remains gated on the
    // rest of the closed effect vocabulary.
    let _trigger_effect_executor = trigger_github.and_then(|github| {
        trigger_workspace_store
            .as_ref()
            .and_then(|workspace_store| {
                github_capability_signer.as_ref().map(|signer| {
                    let baseline_store: Arc<dyn layer2_core::BaselineObjectStore> =
                        Arc::clone(workspace_store) as Arc<dyn layer2_core::BaselineObjectStore>;
                    let checkpoint_store: Arc<dyn layer2_core::CheckpointObjectStore> =
                        Arc::clone(workspace_store) as Arc<dyn layer2_core::CheckpointObjectStore>;
                    Arc::new(trigger::github_effects::GithubEffectExecutor::with_runtime(
                        github,
                        baseline_store,
                        checkpoint_store,
                        Arc::clone(&service),
                        Arc::clone(signer),
                    ))
                })
            })
    });

    // Keeps delegated credentials valid underneath running sessions, and pushes
    // each renewal into the runtimes already using the old value.
    // Per-session credentials are minted on the platform, so this only exists
    // where a platform root is configured. Without one every session falls
    // back to its owner's own token, which is what happened before scoping.
    if let Some(issuer) = issuer {
        auth_state.use_issuer(issuer).await;
        tracing::info!(
            deployment = %config.deployment_id,
            services = ?config.baseline_services,
            "each session gets its own credential, reaching these services and whatever its profile adds"
        );
    }

    renewal::spawn(Arc::clone(&auth_state), Arc::clone(&service));

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
            triggers: trigger_service,
            connections: trigger_connections,
            operations: operation_store,
        }),
    };
    let rest = rest_api_router(
        public_state.clone(),
        Arc::new(layer1_public::rest::admin::AdminState {
            sessions: Arc::clone(&service),
            activity: Arc::clone(&activity),
            users: Arc::clone(&users),
            settings: Arc::clone(&settings),
        }),
    );
    let rate_limits =
        rate_limit::RateLimiters::from_config(config.rate_limits, config.trusted_proxies.clone());

    let app = with_request_context(with_cors(
        Router::new()
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
            // Exact route-class limits see the identity established outside
            // them and run before activity buffers a body or writes MongoDB.
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
            // The extension is outermost so every body extractor sees the
            // chosen ceiling. The activity recorder uses the same constant.
            .layer(DefaultBodyLimit::max(REQUEST_BODY_LIMIT_BYTES)),
        config.cors_origins.clone(),
    ));

    let listener = match tokio::net::TcpListener::bind(&config.bind).await {
        Ok(l) => l,
        Err(e) => {
            tracing::error!(bind = %config.bind, error = %e, "cannot bind");
            std::process::exit(2);
        }
    };
    tracing::info!(bind = %config.bind, "control plane listening");

    if let Err(e) = axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await
    {
        tracing::error!(error = %e, "server stopped with an error");
        std::process::exit(1);
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
    use super::*;

    #[test]
    fn request_span_records_correlation_without_query_credentials() {
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
            let request = Request::get(
                "/api/v1/sessions/sesn_trace_canary/terminal\
                 ?ticket=pty-ticket-secret-488&code=oauth-code-secret-488\
                 &state=oauth-state-secret-488",
            )
            .header("x-request-id", "support-case-488")
            .body(axum::body::Body::empty())
            .expect("request builds");
            let _span = request_span(&request);
        });
        let fields = fields.lock().expect("capture lock is usable");
        assert!(
            fields
                .iter()
                .any(|(name, value)| name == "request_id" && value.contains("support-case-488")),
            "request span must carry the support correlation id: {fields:?}"
        );
        assert!(
            fields.iter().any(|(name, value)| {
                name == "path" && value.contains("/api/v1/sessions/sesn_trace_canary/terminal")
            }),
            "request span must carry the request path: {fields:?}"
        );
        for secret in [
            "pty-ticket-secret-488",
            "oauth-code-secret-488",
            "oauth-state-secret-488",
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
}
