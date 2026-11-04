//! CMA control plane composition while replacement resource routes are rebuilt.

mod activity;
mod agent_access;
mod agent_access_worker;
mod agent_lifecycle;
mod agent_models;
mod auth;
mod bot_setups;
mod commands;
mod composition;
mod config;
mod connections;
mod document_artifacts;
mod document_storage;
mod gateway;
mod health;
mod media_continuations;
mod permissions;
mod profile_authoring;
mod profile_policy;
mod projections;
mod provider_tool_relay;
mod rate_limit;
mod sandbox;
mod shutdown;
mod store;
mod trigger;
mod usage_billing;
mod warm_pools;

use std::{future::IntoFuture, net::SocketAddr, sync::Arc, time::Duration};

use axum::{
    body::Body,
    extract::{Request, State},
    http::{header, HeaderMap, HeaderName, HeaderValue, Method},
    middleware::Next,
    response::Response,
    routing::any,
    Router,
};
use layer1_public::rest::HttpProblem;
use tower_http::{
    request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer},
    trace::{DefaultOnRequest, DefaultOnResponse, TraceLayer},
};
use tracing::Level;

use crate::{
    composition::ControlPlaneComposition,
    config::Config,
    shutdown::{signalled, ShutdownCoordinator},
};

pub(crate) const REQUEST_BODY_LIMIT_BYTES: usize = 2 * 1024 * 1024;
pub(crate) const REST_LINKS: HeaderValue = HeaderValue::from_static(
    "<https://spec.chrono-ai.fun/cma/1.0/conf/rest-core>; rel=\"profile\", <https://spec.chrono-ai.fun/cma/1.0/conf/responses-chat>; rel=\"profile\", </openapi.json>; rel=\"service-desc\"",
);

const DEFAULT_LOG_FILTER: &str = "info";
// Match the Gateway's longest accepted connection before draining workers.
const HTTP_DRAIN_TIMEOUT: Duration = Duration::from_secs(660);
const X_REQUEST_ID: HeaderName = HeaderName::from_static("x-request-id");

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        // Keep container text fields stable for the existing log sink's queries.
        .with_ansi(false)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("RUST_LOG")
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(DEFAULT_LOG_FILTER)),
        )
        .init();

    let arguments: Vec<_> = std::env::args().skip(1).collect();
    if arguments == ["--print-runtime-role"] {
        match std::env::var("CHRONO_MONGODB_DATABASE") {
            Ok(database) => println!("{}", store::isolation::role(&database)),
            Err(_) => {
                eprintln!("CHRONO_MONGODB_DATABASE is required");
                std::process::exit(2)
            }
        }
        return;
    }
    if !arguments.is_empty() {
        eprintln!("usage: cma [--print-runtime-role]");
        std::process::exit(2)
    }
    if let Err(error) = serve().await {
        tracing::error!(error = %error, "CMA stopped");
        std::process::exit(1);
    }
}

async fn serve() -> Result<(), String> {
    let config =
        Config::from_env().map_err(|error| format!("configuration is not usable: {error}"))?;
    let composition = ControlPlaneComposition::build(&config).await?;
    let auth_mode = config.auth_mode_name();
    let app = with_request_context(with_cors(composition.router, config.cors_origins));

    // Do not admit allocations against a newly promoted runtime until its
    // exact default pool has ready capacity (unless an operator disabled it).
    composition
        .native_warm_pools
        .ensure_default()
        .await
        .map_err(|error| format!("native warm pool default is not usable: {error:?}"))?;
    composition
        .native_warm_pools
        .await_default_ready()
        .await
        .map_err(|error| format!("native warm pool startup readiness failed: {error:?}"))?;

    let listener = tokio::net::TcpListener::bind(config.bind)
        .await
        .map_err(|error| format!("cannot bind {}: {error}", config.bind))?;
    composition.sandbox_lifecycle.initialize().await?;
    tracing::info!(
        bind = %config.bind,
        public_url = %config.public_url,
        auth_mode,
        "CMA listening"
    );
    let mut shutdown = ShutdownCoordinator::new();
    shutdown.push(
        composition
            .auth
            .spawn_actor_link_renewal(shutdown.subscribe()),
    );
    shutdown.push(composition.bindings.spawn(shutdown.subscribe()));
    if let Some(bots) = composition.bots {
        shutdown.push(bots.spawn(shutdown.subscribe()));
    }
    if let Some(continuations) = composition.actor_continuations {
        shutdown.push(continuations.spawn(shutdown.subscribe()));
    }
    let worker_signal = shutdown.subscribe();
    shutdown.push(composition.launch_scheduler.spawn(worker_signal));
    shutdown.push(composition.command_scheduler.spawn(shutdown.subscribe()));
    shutdown.push(
        composition
            .sandbox_control_scheduler
            .spawn(shutdown.subscribe()),
    );
    shutdown.push(composition.native_warm_pools.spawn(shutdown.subscribe()));
    shutdown.push(composition.native_prepared.spawn(shutdown.subscribe()));
    shutdown.push(composition.agent_access_worker.spawn(shutdown.subscribe()));
    composition.sandbox_lifecycle.spawn(&mut shutdown);
    shutdown.push(composition.provenance_backfill.spawn(shutdown.subscribe()));
    composition.usage.spawn(&mut shutdown);
    let (http_stop, http_signal) = tokio::sync::watch::channel(false);
    let server_result = {
        let server = axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(signalled(http_signal))
        .into_future();
        tokio::pin!(server);
        tokio::select! {
            result = &mut server => result.map_err(|error| format!("HTTP server failed: {error}")),
            () = shutdown_signal() => {
                // Existing streams still need their workers until HTTP finishes.
                let _ = http_stop.send(true);
                match tokio::time::timeout(HTTP_DRAIN_TIMEOUT, &mut server).await {
                    Ok(result) => result.map_err(|error| format!("HTTP server failed: {error}")),
                    Err(_) => Err("HTTP server exceeded its shutdown drain deadline".into()),
                }
            }
        }
    };
    let aborted = shutdown.drain().await;
    match (server_result, aborted) {
        (Err(error), _) => Err(error),
        (Ok(()), 0) => Ok(()),
        (Ok(()), count) => Err(format!(
            "{count} background workers exceeded shutdown drain"
        )),
    }
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
                        "content-type, authorization, last-event-id, idempotency-key, x-request-id, prefer, x-cma-browser-client, x-cma-document-share-token",
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
                                | "prefer"
                                | "x-cma-browser-client"
                                | "x-cma-document-share-token"
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
