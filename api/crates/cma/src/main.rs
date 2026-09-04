//! CMA control plane composition while replacement resource routes are rebuilt.

mod activity;
mod auth;
mod config;
mod health;
mod permissions;
mod profile_policy;
mod rate_limit;
mod sandbox;
mod store;

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
use layer1_public::rest::{profiles_router, HttpProblem, ProfilesState};
use layer2_core::{
    sandbox::{SandboxOperatorService, SandboxService, StoredSandboxRetirement},
    ActivityService, OperationStore, PlatformSettingsService, UserDirectory,
};
use tower_http::{
    request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer},
    trace::{DefaultOnRequest, DefaultOnResponse, TraceLayer},
};
use tracing::Level;

use crate::{
    activity::ActivityRecorder,
    auth::{keys::KeyIssuer, AuthMode, AuthState, SignInRecords},
    config::Config,
    profile_policy::profile_route_policy,
    rate_limit::RateLimiters,
    sandbox::{LiveSandboxPolicy, ManagedSandboxCredentials},
    store::{DefaultProfileSpec, MongoStores},
};

pub(crate) const REQUEST_BODY_LIMIT_BYTES: usize = 2 * 1024 * 1024;
pub(crate) const REST_LINKS: HeaderValue = HeaderValue::from_static(
    "<https://spec.chrono-ai.fun/cma/1.0/conf/rest-core>; rel=\"profile\", </openapi.json>; rel=\"service-desc\"",
);

const DEFAULT_LOG_FILTER: &str = "info";
const X_REQUEST_ID: HeaderName = HeaderName::from_static("x-request-id");

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("RUST_LOG")
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(DEFAULT_LOG_FILTER)),
        )
        .init();

    if let Err(error) = serve().await {
        tracing::error!(error = %error, "CMA stopped");
        std::process::exit(1);
    }
}

async fn serve() -> Result<(), String> {
    let config =
        Config::from_env().map_err(|error| format!("configuration is not usable: {error}"))?;
    let stores = Arc::new(
        MongoStores::connect(
            &config.mongodb_uri,
            &config.mongodb_database,
            &config.deployment_id,
            &config.auth_encryption_key,
            config.activity_retention,
        )
        .await
        .map_err(|error| format!("durable stores are not usable: {error}"))?,
    );
    let activity = ActivityService::new(stores.activity.clone());
    let users = UserDirectory::new(stores.users.clone());
    let profile_policy = profile_route_policy();
    stores
        .sandbox_profiles
        .seed_default_profiles(
            &DefaultProfileSpec {
                cpu_millis: config.default_profiles.cpu_millis,
                memory_mib: config.default_profiles.memory_mib,
                agent_model: config.default_profiles.agent_model.clone(),
                agent_effort: config.default_profiles.agent_effort,
            },
            profile_policy.capacity_bounds,
            &profile_policy.secret_patterns,
        )
        .await
        .map_err(|error| format!("default profiles are not usable: {error}"))?;
    // #809 composes the owner-facing Sandbox services. Opening the stores here
    // already enforces indexes and schema compatibility before serving.
    let retirement = Arc::new(StoredSandboxRetirement::new(stores.sandbox_model.clone()));
    let sandbox_operators = Arc::new(SandboxOperatorService::new(
        stores.sandbox_model.clone(),
        stores.sandbox_profiles.clone(),
        retirement,
        Arc::clone(&activity),
    ));
    let settings = Arc::new(PlatformSettingsService::new(
        stores.settings.clone(),
        Arc::clone(&activity),
    ));
    let sandbox_policy = Arc::new(LiveSandboxPolicy::new(
        Arc::clone(&settings),
        config.baseline_services.clone(),
    ));
    let _sandbox_service = SandboxService::with_policy_source(
        stores.sandbox_model.clone(),
        stores.sandbox_profiles.clone(),
        sandbox_policy,
    );
    let admin = layer1_public::rest::admin::AdminState::new(
        Arc::clone(&settings),
        sandbox_operators,
        Arc::clone(&activity),
    );
    let operations: Arc<dyn OperationStore> = stores.operations.clone();
    let profiles = ProfilesState::new(
        stores.sandbox_profiles.clone(),
        operations,
        Arc::clone(&settings),
        profile_policy,
    );
    let recorder = ActivityRecorder::new(Arc::clone(&activity));
    let auth = AuthState::with_store_and_nyxid_public_url(
        config.auth_mode.clone(),
        Some(SignInRecords { users, activity }),
        stores.auth.clone(),
        config.nyxid_public_url.clone(),
    );
    let _sandbox_credentials = match &config.auth_mode {
        AuthMode::Provider { base_url, .. } => Some(ManagedSandboxCredentials::new(
            Arc::new(KeyIssuer::new(
                reqwest::Client::builder()
                    .timeout(std::time::Duration::from_secs(15))
                    .redirect(reqwest::redirect::Policy::none())
                    .build()
                    .expect("the rustls HTTP client builds"),
                base_url,
                &config.deployment_id,
            )),
            Arc::clone(&auth),
            Arc::clone(&stores.sandbox_credentials),
        )),
        AuthMode::Trusted { .. } => None,
    };
    if !auth.requires_credential() {
        tracing::warn!(
            "running with a trusted fixed subject; this mode is for local development only"
        );
    }

    let limits = RateLimiters::from_config(config.rate_limits, config.trusted_proxies.clone());
    let resource_routes =
        profiles_router(profiles).merge(layer1_public::rest::admin::router(admin));
    let public = rest_router(resource_routes)
        .merge(auth::router(Arc::clone(&auth)))
        .layer(axum::middleware::from_fn_with_state(
            recorder,
            activity::record,
        ))
        .layer(axum::middleware::from_fn_with_state(
            Arc::clone(&limits),
            rate_limit::enforce,
        ))
        .layer(axum::middleware::from_fn_with_state(
            auth,
            auth::attach_subject,
        ));
    let app = Router::new()
        .merge(public)
        .merge(health::router(stores.clone()))
        .layer(axum::middleware::from_fn_with_state(
            limits,
            rate_limit::admit,
        ))
        .layer(DefaultBodyLimit::max(REQUEST_BODY_LIMIT_BYTES));
    let auth_mode = config.auth_mode_name();
    let app = with_request_context(with_cors(app, config.cors_origins));

    let listener = tokio::net::TcpListener::bind(config.bind)
        .await
        .map_err(|error| format!("cannot bind {}: {error}", config.bind))?;
    tracing::info!(
        bind = %config.bind,
        public_url = %config.public_url,
        auth_mode,
        "CMA listening"
    );
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await
    .map_err(|error| format!("HTTP server failed: {error}"))
}

fn rest_router(resource_routes: Router) -> Router {
    layer1_public::rest::router()
        .merge(resource_routes)
        .route("/api/v1", any(|| async { HttpProblem::NotFound }))
        .route("/api/v1/", any(|| async { HttpProblem::NotFound }))
        .route("/api/v1/{*path}", any(|| async { HttpProblem::NotFound }))
        .method_not_allowed_fallback(|| async { HttpProblem::MethodNotAllowed })
        .layer(axum::middleware::from_fn(rest_links))
}

async fn rest_links(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    response.headers_mut().append(header::LINK, REST_LINKS);
    response
}

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

fn with_request_context(router: Router) -> Router {
    router
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
    request.headers_mut().remove(X_REQUEST_ID);
    if let Some(normalized) = normalized {
        request.headers_mut().insert(X_REQUEST_ID, normalized);
    }
    next.run(request).await
}

fn request_span(request: &Request) -> tracing::Span {
    let request_id = request
        .headers()
        .get(X_REQUEST_ID)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("<non-text request id>");
    tracing::info_span!(
        "http.request",
        request_id = %request_id,
        method = %request.method(),
        path = %request.uri().path(),
    )
}

async fn shutdown_signal() {
    let ctrl_c = async {
        if tokio::signal::ctrl_c().await.is_err() {
            std::future::pending::<()>().await;
        }
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {},
        () = terminate => {},
    }
}

#[cfg(test)]
#[path = "main_tests.rs"]
mod tests;
