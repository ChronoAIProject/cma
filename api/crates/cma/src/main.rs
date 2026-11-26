//! CMA control plane composition while replacement resource routes are rebuilt.
// Tests drive raw driver sessions to construct the faults the guarded
// terminal commands defend against (#3288).
#![cfg_attr(test, allow(clippy::disallowed_methods))]

// Avoid libc allocator retention and contention across async worker threads.
#[global_allocator]
static GLOBAL_ALLOCATOR: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[cfg(test)]
#[path = "../../cmaeg/tests/support/global_allocator.rs"]
mod global_allocator_tests;

mod activity;
mod agent_access;
mod agent_access_worker;
mod agent_lifecycle;
mod agent_manager;
mod agent_models;
mod auth;
mod background;
mod binding_diagnostics;
mod bot_setups;
mod commands;
mod composition;
mod config;
mod connections;
mod control_admission;
#[cfg(test)]
mod dependency_test_support;
mod diagnostic_format;
mod diagnostic_request;
mod document_artifacts;
mod document_storage;
mod expensive_reads;
mod gateway;
mod health;
#[allow(dead_code)] // Retention preparation has no qualified production deletion provider.
mod home_backup_expiry;
mod http_serving;
mod image_manager_retirement;
mod media_continuations;
use cma_am_auth::permissions;
mod producer_token;
mod profile_authoring;
mod profile_policy;
mod projections;
mod provider_tool_relay;
mod public_streams;
#[cfg(feature = "qualification-hooks")]
mod qualification;
mod rate_limit;
mod read_telemetry;
mod readiness_diagnostics;
mod retirement_archive;
mod runtime_pointer;
mod sandbox;
mod sandbox_metrics;
mod sandbox_usage_export;
mod shutdown;
mod startup;
mod startup_timing;
mod store;
mod telemetry;
mod trigger;
mod usage_billing;
mod warm_pools;
mod worker_supervision;

use std::sync::Arc;

use startup::serve;

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

pub(crate) const REQUEST_BODY_LIMIT_BYTES: usize = 2 * 1024 * 1024;
pub(crate) const REST_LINKS: HeaderValue = HeaderValue::from_static(
    "<https://spec.chrono-ai.fun/cma/1.0/conf/rest-core>; rel=\"profile\", <https://spec.chrono-ai.fun/cma/1.0/conf/responses-chat>; rel=\"profile\", </openapi.json>; rel=\"service-desc\"",
);

const DEFAULT_LOG_FILTER: &str = "info";
const X_REQUEST_ID: HeaderName = HeaderName::from_static("x-request-id");

fn main() {
    let mut arguments = std::env::args_os().skip(1);
    if arguments.next().as_deref() == Some(std::ffi::OsStr::new("--allocator-self-test")) {
        // Qualify this executable's allocator before Tokio starts threads or
        // any serving, configuration, metrics or database composition runs.
        if arguments.next().is_some() {
            eprintln!("usage: cma --allocator-self-test");
            std::process::exit(2);
        }
        if let Err(error) = cma_allocator_qualification::run() {
            eprintln!("allocator-self-test outcome=failed error={error}");
            std::process::exit(1);
        }
        return;
    }
    run();
}

#[tokio::main]
async fn run() {
    let arguments: Vec<_> = std::env::args().skip(1).collect();
    if arguments.first().is_some_and(|value| {
        matches!(
            value.as_str(),
            "--measured-capture"
                | "--measured-preview"
                | "--measured-freeze"
                | "--measured-evidence-read"
                | "--measured-period-read"
                | "--measured-period-replay"
                | "--measured-index-backfill"
                | "--measured-index-certify"
        )
    }) {
        match store::measured_capture::command::run(&arguments).await {
            Ok(receipt) => println!("{receipt}"),
            Err(reason) => {
                eprintln!("{reason}");
                std::process::exit(2);
            }
        }
        return;
    }
    if store::turn_budget_inventory::is_command(&arguments) {
        match store::turn_budget_inventory::command(&arguments).await {
            Ok(receipt) => println!("{receipt}"),
            Err(reason) => {
                eprintln!("{reason}");
                std::process::exit(2);
            }
        }
        return;
    }
    if store::home_backups::evidence_export::cli::is_command(&arguments) {
        match store::home_backups::evidence_export::cli::command(&arguments).await {
            Ok(receipt) => println!("{receipt}"),
            Err(reason) => {
                eprintln!("{reason}");
                std::process::exit(2);
            }
        }
        return;
    }
    if store::sandbox_usage_export::run_repair_command(&arguments).await {
        return;
    }
    if retirement_archive::is_command(&arguments) {
        match retirement_archive::command(&arguments).await {
            Ok(summary) => println!("{summary}"),
            Err(reason) => {
                eprintln!("{reason}");
                std::process::exit(2);
            }
        }
        return;
    }
    if let Some(result) = store::agent_inspection_park::command(&arguments).await {
        match result {
            Ok(receipt) => println!("{receipt}"),
            Err(reason) => {
                eprintln!("{reason}");
                std::process::exit(2);
            }
        }
        return;
    }
    if arguments.len() == 3 && arguments[0] == "--ledger-collection-observation" {
        if let Err(error) = store::ledger_collection::command(
            std::path::Path::new(&arguments[1]),
            std::path::Path::new(&arguments[2]),
        )
        .await
        {
            eprintln!("{error}");
            std::process::exit(2);
        }
        return;
    }
    if arguments
        .first()
        .is_some_and(|argument| argument == "--task-audit")
    {
        if arguments.len() != 3 {
            eprintln!("usage: cma --task-audit PRIVATE_SELECTION PRIVATE_NEW_FILE");
            std::process::exit(2);
        }
        if let Err(error) = store::task_audit::command(
            std::path::Path::new(&arguments[1]),
            std::path::Path::new(&arguments[2]),
        )
        .await
        {
            eprintln!("task audit refused: {error}");
            std::process::exit(2);
        }
        return;
    }
    if arguments.len() == 3 && arguments[0] == "--ledger-census" {
        if let Err(error) = store::ledger_census::command(
            std::path::Path::new(&arguments[1]),
            std::path::Path::new(&arguments[2]),
        )
        .await
        {
            eprintln!("{error}");
            std::process::exit(2);
        }
        return;
    }
    if arguments.len() == 3 && arguments[0] == "--ledger-retirement-snapshot" {
        if let Err(error) =
            store::ledger_retirement::command(&arguments[1], std::path::Path::new(&arguments[2]))
                .await
        {
            eprintln!("{error}");
            std::process::exit(2);
        }
        return;
    }

    tracing_subscriber::fmt()
        // Keep container text fields stable for the existing log sink's queries.
        .with_ansi(false)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("RUST_LOG")
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(DEFAULT_LOG_FILTER)),
        )
        .event_format(diagnostic_format::text())
        .init();

    if document_artifacts::quota_cli::is_command(&arguments) {
        match document_artifacts::quota_cli::command(&arguments).await {
            Ok(result) => println!("{result}"),
            Err(reason) => {
                eprintln!("document quota command refused: {reason}");
                std::process::exit(2);
            }
        }
        return;
    }
    if store::migrations::is_command(&arguments) {
        store::migrations::run_command_line(&arguments).await;
        return;
    }
    if store::segment_operations::is_command(&arguments) {
        match store::segment_operations::command(&arguments).await {
            Ok(result) => println!("{result}"),
            Err(reason) => {
                eprintln!("Agent log operation refused: {reason}");
                std::process::exit(2);
            }
        }
        return;
    }
    if arguments == ["--print-runtime-role"] {
        match std::env::var("CHRONO_MONGODB_DATABASE") {
            Ok(database) => println!(
                "{}",
                store::isolation::replicas::role(
                    &database,
                    std::env::var("CHRONO_AGENT_MANAGER_REPLICAS_ENABLED").as_deref() == Ok("true"),
                )
            ),
            Err(_) => {
                eprintln!("CHRONO_MONGODB_DATABASE is required");
                std::process::exit(2)
            }
        }
        return;
    }
    if let Some(result) = store::backfill_progress_command(&arguments).await {
        match result {
            Ok(result) => println!("{result}"),
            Err(message) => {
                eprintln!("{message}");
                std::process::exit(2);
            }
        }
        return;
    }
    if arguments
        .first()
        .is_some_and(|argument| argument == "--runtime-pointer")
    {
        if let Err(error) = runtime_pointer::operator::command(&arguments).await {
            eprintln!("{error}");
            std::process::exit(2);
        }
        return;
    }
    #[cfg(feature = "qualification-hooks")]
    if let Some(options) = qualification::startup_options(&arguments) {
        let options = options.unwrap_or_else(|message| {
            eprintln!("{message}");
            std::process::exit(2);
        });
        if let Err(error) = startup::serve_qualified(options).await {
            tracing::error!(error = %error, "CMA stopped");
            std::process::exit(1);
        }
        return;
    }
    if !arguments.is_empty() && arguments != ["--agent-manager-drain"] {
        eprintln!("usage: cma [--index-catalog | --index-preflight NAMES | --index-size NAMES | --index-verify NAMES | --index-build NAME CHECKSUM IMAGE [SECONDS] | --index-status NAME | --index-abort NAME CHECKSUM IMAGE FENCE | --activity-index-size | --activity-index-build CHECKSUM IMAGE [TIMEOUT_SECONDS] | --print-runtime-role | --agent-manager-drain | --migration-plan | --migration-status | --migration-apply CHECKSUM IMAGE | --print-migration-role | --ledger-retirement-snapshot SANDBOX PRIVATE_NEW_FILE | --ledger-census PRIVATE_REQUEST PRIVATE_NEW_FILE | --measured-capture SANDBOX FROM TO | --measured-preview SAMPLE FROM TO | --measured-freeze PRIVATE_REQUEST PRIVATE_NEW_FILE | --measured-evidence-read PRIVATE_REQUEST PRIVATE_NEW_FILE | --measured-period-read PRIVATE_REQUEST PRIVATE_NEW_FILE | --measured-period-replay PRIVATE_REQUEST PRIVATE_NEW_FILE | --measured-index-backfill [EXPECTED_PREDECESSOR_SHA256] | --measured-index-certify | --home-backup-catalog-export PRIVATE_NEW_FILE | --task-audit PRIVATE_SELECTION PRIVATE_NEW_FILE | --allocator-self-test]");
        std::process::exit(2)
    }
    if let Err(error) = serve().await {
        tracing::error!(error = %error, "CMA stopped");
        std::process::exit(1);
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
        .layer(axum::middleware::from_fn(telemetry::http::observe))
        .layer(PropagateRequestIdLayer::new(X_REQUEST_ID))
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(diagnostic_request::request_span)
                .on_request(DefaultOnRequest::new().level(Level::INFO))
                .on_response(DefaultOnResponse::new().level(Level::INFO)),
        )
        .layer(axum::middleware::from_fn(diagnostic_request::scope))
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

#[cfg(test)]
mod public_signup_abuse_tests;

#[cfg(test)]
#[path = "main_tests.rs"]
mod tests;
